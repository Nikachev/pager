//! Single-Slot USB Mass Storage (UF2) Bootloader for nRF52840

#![no_std]
#![no_main]

#[cfg(any(
    all(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"),
    not(any(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"))
))]
compile_error!("select exactly one Pager board feature");

mod double_tap;
mod fat16;
mod led;
mod manifest;
mod memory_map;
mod msc_flash;
mod public_key;
mod scsi;
mod uf2;

use cortex_m_rt::entry;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::nvmc::Nvmc;
use memory_map::{image_len_is_valid, FIRMWARE_START, MANIFEST_SIZE};
use msc_flash::Uf2FlashEngine;
use nrf_usbd::{UsbPeripheral, Usbd};
use sha2::{Digest, Sha256};

use embassy_time::{Duration, Instant};
use usb_device::bus::UsbBusAllocator;
use usb_device::prelude::*;
use usbd_storage::subclass::scsi::Scsi;

pub use led::{BootReason, LedIndicator};

const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;
const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const USBD_ENABLE: *mut u32 = 0x4002_7500 as *mut u32;
const USBD_PULLUP: *mut u32 = 0x4002_7504 as *mut u32;

struct Nrf52840Usbd;
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    reset_after_fault()
}

fn reset_after_fault() -> ! {
    double_tap::mark_fault();
    cortex_m::peripheral::SCB::sys_reset()
}

fn start_hfclk() {
    let clock_regs = 0x4000_0000 as *mut u32;
    unsafe {
        // TASKS_HFCLKSTART = 1 (0x40000000)
        core::ptr::write_volatile(clock_regs, 1);
        // Wait EVENTS_HFCLKSTARTED (0x40000100)
        while core::ptr::read_volatile(clock_regs.add(64)) == 0 {}
        core::ptr::write_volatile(clock_regs.add(64), 0);
    }
}

fn init_nrf52840_usb_power() {
    let power_base = 0x4000_0000 as *mut u32;
    unsafe {
        // Wait for USB regulator OUTPUTRDY (USBREGSTATUS & 0x02 != 0)
        let usbregstatus = power_base.add(0x438 / 4);
        let mut retries = 0;
        while (core::ptr::read_volatile(usbregstatus) & 0x02) == 0 {
            cortex_m::asm::nop();
            retries += 1;
            if retries > 100_000 {
                break;
            }
        }
    }
}

fn factory_usb_serial() -> heapless::String<16> {
    let device0 = unsafe { core::ptr::read_volatile(0x1000_0060 as *const u32) };
    let device1 = unsafe { core::ptr::read_volatile(0x1000_0064 as *const u32) };
    let mut serial = heapless::String::new();
    let _ = core::fmt::write(&mut serial, format_args!("{device1:08X}{device0:08X}"));
    serial
}

fn feed_inherited_watchdog() {
    const WDT_BASE: usize = 0x4001_0000;
    const RELOAD_MAGIC: u32 = 0x6E52_4635;
    unsafe {
        let running = core::ptr::read_volatile((WDT_BASE + 0x400) as *const u32) != 0;
        if !running {
            return;
        }
        let enabled = core::ptr::read_volatile((WDT_BASE + 0x508) as *const u32);
        for register in 0..8 {
            if enabled & (1 << register) != 0 {
                core::ptr::write_volatile(
                    (WDT_BASE + 0x600 + register * 4) as *mut u32,
                    RELOAD_MAGIC,
                );
            }
        }
    }
}

use cortex_m_rt::exception;

#[exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    double_tap::mark_fault();
    cortex_m::peripheral::SCB::sys_reset()
}

#[entry]
fn main() -> ! {
    // 1. Check double-tap / DFU reset trigger BEFORE initializing Embassy peripherals
    let reset_reason = double_tap::capture_reset_reason();
    let fault_reset = double_tap::take_fault();
    let double_tap = !fault_reset && double_tap::check_and_set_double_tap(reset_reason);

    start_hfclk();
    init_nrf52840_usb_power();
    let p = embassy_nrf::init(Default::default());

    #[cfg(feature = "board-nice-nano-v2")]
    let led_pin = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    #[cfg(feature = "board-xiao-nrf52840")]
    let led_pin = Output::new(p.P0_26, Level::High, OutputDrive::Standard);
    let mut indicator = LedIndicator::new(led_pin);

    // 2. Validate existing firmware at FIRMWARE_START
    let valid_fw = validate_existing_firmware();

    // 3. Determine if we must stay in Bootloader / DFU mode
    let boot_reason = if fault_reset {
        Some(BootReason::Fault)
    } else if double_tap {
        Some(BootReason::UserRequest)
    } else if !valid_fw.valid_vector {
        Some(BootReason::NoFirmware)
    } else if !valid_fw.valid_sig {
        Some(BootReason::SignatureError)
    } else if !valid_fw.valid_hash {
        Some(BootReason::IntegrityError)
    } else {
        None
    };

    if let Some(reason) = boot_reason {
        double_tap::clear_double_tap();

        // Force hardware USB detach and reset USBD peripheral
        unsafe {
            let usbd_base = 0x4002_7000 as *mut u32;
            core::ptr::write_volatile(usbd_base.add(0x504 / 4), 0); // USBPULLUP = 0
            core::ptr::write_volatile(usbd_base.add(0x500 / 4), 0); // ENABLE = 0
            cortex_m::asm::delay(50 * 64_000); // 50ms disconnect delay
        }

        // Synchronous USBD driver for 100% standalone reliability
        static BUS_ALLOC: static_cell::StaticCell<UsbBusAllocator<Usbd<Nrf52840Usbd>>> =
            static_cell::StaticCell::new();
        static SCSI_BUF: static_cell::StaticCell<[u8; 18432]> = static_cell::StaticCell::new();

        let bus_alloc = BUS_ALLOC.init(UsbBusAllocator::new(Usbd::new(Nrf52840Usbd)));
        let scsi_buf = SCSI_BUF.init([0u8; 18432]);
        let scsi_buf_ref: &'static mut [u8] = scsi_buf.as_mut_slice();

        let mut msc_class = match Scsi::new(bus_alloc, 64, 0, scsi_buf_ref) {
            Ok(class) => class,
            Err(_) => reset_after_fault(),
        };

        let usb_serial = factory_usb_serial();
        let usb_builder = UsbDeviceBuilder::new(bus_alloc, UsbVidPid(0x239A, 0x0029));
        let usb_builder = match usb_builder.strings(&[StringDescriptors::default()
            .manufacturer("Nikachev")
            .product("Pager Boot Drive")
            .serial_number(usb_serial.as_str())])
        {
            Ok(builder) => builder,
            Err(_) => reset_after_fault(),
        };
        let usb_builder = usb_builder.device_class(0x00);
        let mut usb_dev = match usb_builder.max_packet_size_0(64) {
            Ok(builder) => builder.build(),
            Err(_) => reset_after_fault(),
        };

        // Re-enable USBD peripheral hardware and D+ pullup resistor (USBPULLUP = 1)
        unsafe {
            let usbd_base = 0x4002_7000 as *mut u32;
            core::ptr::write_volatile(usbd_base.add(0x500 / 4), 1); // ENABLE = 1
            core::ptr::write_volatile(usbd_base.add(0x504 / 4), 1); // USBPULLUP = 1
        }

        let nvmc = Nvmc::new(p.NVMC);
        let mut flash_engine = Uf2FlashEngine::new(nvmc);

        let started_at = Instant::now();
        let mut reset_at: Option<Instant> = None;

        loop {
            // Poll USB at full hardware speed for zero-latency Bulk transfers
            let _ = usb_dev.poll(&mut [&mut msc_class]);
            let _ = msc_class.poll(|cmd| {
                flash_engine.handle_scsi_command(cmd);
            });

            // A watchdog survives reset and cannot be stopped on nRF52840.
            // Feed every reload register enabled by the previous application.
            feed_inherited_watchdog();

            let now = Instant::now();
            indicator.tick_nonblocking(now.duration_since(started_at).as_millis() as u32, reason);

            if flash_engine.take_reset_pending() {
                // Give the MSC transport time to send the successful CSW before reset.
                reset_at = Some(now + Duration::from_millis(100));
            }
            if reset_at.is_some_and(|deadline| now >= deadline) {
                cortex_m::peripheral::SCB::sys_reset();
            }

            // Approximate five-minute timeout based on RTC, independent of USB load.
            if now.duration_since(started_at) >= Duration::from_secs(300) {
                let fw = validate_existing_firmware();
                if fw.valid_vector && fw.valid_sig && fw.valid_hash {
                    cortex_m::peripheral::SCB::sys_reset();
                }
            }
        }
    } else {
        double_tap::clear_double_tap();
        jump(FIRMWARE_START + MANIFEST_SIZE);
    }
}

struct FirmwareValidationResult {
    valid_vector: bool,
    valid_sig: bool,
    valid_hash: bool,
}

fn validate_existing_firmware() -> FirmwareValidationResult {
    let start_ptr = FIRMWARE_START as *const u8;
    let manifest_ptr = start_ptr as *const manifest::Manifest;
    // Generated firmware_start is in flash, with a 256-byte manifest reserve;
    // the shared Manifest occupies its first 112 bytes.
    let manifest = unsafe { core::ptr::read_unaligned(manifest_ptr) };

    if manifest.magic != manifest::MAGIC || manifest.image_len == 0 {
        return FirmwareValidationResult {
            valid_vector: false,
            valid_sig: false,
            valid_hash: false,
        };
    }

    if !image_len_is_valid(manifest.image_len) {
        return FirmwareValidationResult {
            valid_vector: false,
            valid_sig: false,
            valid_hash: false,
        };
    }

    let image_len = manifest.image_len as usize;
    let image_start = FIRMWARE_START + MANIFEST_SIZE;
    // image_len_is_valid() checked alignment and the complete range against
    // storage_start before constructing this flash slice.
    let image_slice = unsafe { core::slice::from_raw_parts(image_start as *const u8, image_len) };

    let valid_vector = valid_vector_table(image_slice, image_start);

    let signed_msg = manifest.signed_message();
    let valid_sig = public_key::verify_signature(&signed_msg, &manifest.signature);

    let computed_digest = Sha256::digest(image_slice);
    let valid_hash = computed_digest.as_slice() == manifest.digest;

    FirmwareValidationResult {
        valid_vector,
        valid_sig,
        valid_hash,
    }
}

fn valid_vector_table(image: &[u8], image_start: u32) -> bool {
    pager_bootloader_core::codec::validate_vector(image, image_start).is_ok()
}

fn jump(image_start: u32) -> ! {
    unsafe {
        cortex_m::interrupt::disable();

        // Embassy configures SysTick during peripheral initialization. Restore
        // timer and USB state to their reset values before handing over.
        core::ptr::write_volatile(SYST_CSR, 0);
        core::ptr::write_volatile(USBD_PULLUP, 0);
        core::ptr::write_volatile(USBD_ENABLE, 0);

        // Disable all NVIC interrupts and clear any pending — clean slate for firmware
        for i in 0u32..8 {
            core::ptr::write_volatile((0xE000_E180u32 + i * 4) as *mut u32, 0xFFFF_FFFF); // ICER
            core::ptr::write_volatile((0xE000_E280u32 + i * 4) as *mut u32, 0xFFFF_FFFF);
            // ICPR
        }

        core::ptr::write_volatile(SCB_VTOR, image_start);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();

        // Restore PRIMASK=0 to match hardware-reset state before jumping
        cortex_m::interrupt::enable();
        cortex_m::asm::bootload(image_start as *const u32)
    }
}
