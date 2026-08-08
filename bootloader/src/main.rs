//! Single-Slot USB Mass Storage (UF2) Bootloader for nRF52840

#![no_std]
#![no_main]

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
use ed25519_dalek::{Signature, VerifyingKey};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::nvmc::Nvmc;
use memory_map::{FIRMWARE_END, FIRMWARE_START, MANIFEST_SIZE};
use msc_flash::Uf2FlashEngine;
use nrf_usbd::{UsbPeripheral, Usbd};
use sha2::{Digest, Sha256};

use usb_device::bus::UsbBusAllocator;
use usb_device::prelude::*;
use usbd_storage::subclass::scsi::Scsi;

pub use led::{BootReason, LedIndicator};

const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;

struct Nrf52840Usbd;
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
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

use cortex_m_rt::exception;

#[exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

#[entry]
fn main() -> ! {
    // 1. Check double-tap / DFU reset trigger BEFORE initializing Embassy peripherals
    let double_tap = double_tap::check_and_set_double_tap();

    start_hfclk();
    init_nrf52840_usb_power();
    let p = embassy_nrf::init(Default::default());

    let led_pin = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    let mut indicator = LedIndicator::new(led_pin);

    // 2. Validate existing firmware at FIRMWARE_START
    let valid_fw = validate_existing_firmware();

    // 3. Determine if we must stay in Bootloader / DFU mode
    let boot_reason = if double_tap {
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

        let mut msc_class = Scsi::new(bus_alloc, 64, 0, scsi_buf_ref).unwrap();

        let mut usb_dev = UsbDeviceBuilder::new(bus_alloc, UsbVidPid(0x239A, 0x0029))
            .strings(&[StringDescriptors::default()
                .manufacturer("Nikachev")
                .product("Pager Boot Drive")
                .serial_number("12345678")])
            .unwrap()
            .device_class(0x00)
            .max_packet_size_0(64)
            .unwrap()
            .build();

        // Re-enable USBD peripheral hardware and D+ pullup resistor (USBPULLUP = 1)
        unsafe {
            let usbd_base = 0x4002_7000 as *mut u32;
            core::ptr::write_volatile(usbd_base.add(0x500 / 4), 1); // ENABLE = 1
            core::ptr::write_volatile(usbd_base.add(0x504 / 4), 1); // USBPULLUP = 1
        }

        let nvmc = Nvmc::new(p.NVMC);
        let mut flash_engine = Uf2FlashEngine::new(nvmc);

        let mut loop_counter: u32 = 0;
        let mut tick_ms: u32 = 0;

        loop {
            // Poll USB at full hardware speed for zero-latency Bulk transfers
            let _ = usb_dev.poll(&mut [&mut msc_class]);
            let _ = msc_class.poll(|cmd| {
                flash_engine.handle_scsi_command(cmd);
            });

            loop_counter = loop_counter.wrapping_add(1);

            // Feed watchdog if active from main application
            unsafe {
                core::ptr::write_volatile(0x4001_0600 as *mut u32, 0x6E52_4635);
            }

            if (loop_counter & 0x1FFF) == 0 {
                tick_ms = tick_ms.wrapping_add(1);
                indicator.tick_nonblocking(tick_ms, reason);

                // DFU Auto-Timeout: If idle for 5 minutes (300,000 ms), boot existing firmware if valid
                if tick_ms >= 300_000 {
                    let fw = validate_existing_firmware();
                    if fw.valid_vector && fw.valid_sig && fw.valid_hash {
                        cortex_m::peripheral::SCB::sys_reset();
                    }
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
    let manifest = unsafe { core::ptr::read_unaligned(manifest_ptr) };

    if manifest.magic != manifest::MAGIC || manifest.image_len == 0 {
        return FirmwareValidationResult {
            valid_vector: false,
            valid_sig: false,
            valid_hash: false,
        };
    }

    let image_len = manifest.image_len as usize;
    let max_len = (FIRMWARE_END - FIRMWARE_START) as usize;
    if image_len > max_len {
        return FirmwareValidationResult {
            valid_vector: false,
            valid_sig: false,
            valid_hash: false,
        };
    }

    let image_start = FIRMWARE_START + MANIFEST_SIZE;
    let image_slice = unsafe { core::slice::from_raw_parts(image_start as *const u8, image_len) };

    let valid_vector = valid_vector_table(image_slice, image_start);

    let signed_msg = manifest.signed_message();
    let valid_sig = public_key::FIRMWARE_SIGNING_PUBLIC_KEYS
        .iter()
        .any(|key_bytes| {
            VerifyingKey::from_bytes(key_bytes).is_ok_and(|key| {
                key.verify_strict(&signed_msg, &Signature::from_bytes(&manifest.signature))
                    .is_ok()
            })
        });

    let computed_digest = Sha256::digest(image_slice);
    let valid_hash = computed_digest.as_slice() == manifest.digest;

    FirmwareValidationResult {
        valid_vector,
        valid_sig,
        valid_hash,
    }
}

fn valid_vector_table(image: &[u8], image_start: u32) -> bool {
    if image.len() < 8 {
        return false;
    }
    let initial_sp = u32::from_le_bytes(image[..4].try_into().unwrap());
    let reset = u32::from_le_bytes(image[4..8].try_into().unwrap());
    (0x2000_0000..=0x2004_0000).contains(&initial_sp)
        && (reset & 1) == 1
        && (image_start..image_start + image.len() as u32).contains(&(reset & !1))
}

fn jump(image_start: u32) -> ! {
    unsafe {
        cortex_m::interrupt::disable();

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
