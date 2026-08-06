//! Single-Slot USB Mass Storage (UF2) Bootloader for nRF52840

#![no_std]
#![no_main]

mod double_tap;
mod led;
mod manifest;
mod msc_flash;
mod public_key;
mod raw_uf2;
mod uf2;

use cortex_m_rt::entry;
use ed25519_dalek::{Signature, VerifyingKey};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::nvmc::Nvmc;
use msc_flash::{Uf2FlashEngine, FIRMWARE_END, FIRMWARE_START};
use nrf_usbd::{UsbPeripheral, Usbd};
use raw_uf2::RawUf2Class;
use sha2::{Digest, Sha256};
use static_cell::StaticCell;
use uf2::Uf2Block;
use usb_device::bus::UsbBusAllocator;
use usb_device::prelude::*;

pub use led::{BootReason, LedIndicator};

const SCB_VTOR: *mut u32 = 0xE000_ED08 as *mut u32;
pub const MANIFEST_SIZE: u32 = 256;

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

#[entry]
fn main() -> ! {
    start_hfclk();
    let p = embassy_nrf::init(Default::default());

    let led_pin = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    let mut indicator = LedIndicator::new(led_pin);

    // 1. Check double-tap reset trigger
    let double_tap = double_tap::check_and_set_double_tap();

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
        if !double_tap {
            cortex_m::asm::delay(500 * 64_000);
        }
        double_tap::clear_double_tap();

        // Force hardware USB detach and reset USBD peripheral
        unsafe {
            let usbd_base = 0x4002_7000 as *mut u32;
            core::ptr::write_volatile(usbd_base.add(0x504 / 4), 0); // USBPULLUP = 0
            core::ptr::write_volatile(usbd_base.add(0x500 / 4), 0); // ENABLE = 0
            cortex_m::asm::delay(50 * 64_000); // 50ms disconnect delay
        }

        // Synchronous USBD driver for 100% standalone reliability
        static mut BUS_ALLOC_STORAGE: core::mem::MaybeUninit<UsbBusAllocator<Usbd<Nrf52840Usbd>>> =
            core::mem::MaybeUninit::uninit();
        let bus_alloc: &'static UsbBusAllocator<Usbd<Nrf52840Usbd>> = unsafe {
            BUS_ALLOC_STORAGE.write(UsbBusAllocator::new(Usbd::new(Nrf52840Usbd)))
        };

        let mut uf2_class = RawUf2Class::new(bus_alloc);

        let mut usb_dev = UsbDeviceBuilder::new(bus_alloc, UsbVidPid(0x1209, 0x0001))
            .strings(&[StringDescriptors::default()
                .manufacturer("Nikachev")
                .product("Pager Bootloader")
                .serial_number("12345678")])
            .unwrap()
            .device_class(0xFF)
            .max_packet_size_0(64)
            .unwrap()
            .build();

        let nvmc = Nvmc::new(p.NVMC);
        let mut flash_engine = Uf2FlashEngine::new(nvmc);

        let mut block_buf = [0u8; 512];
        let mut block_off = 0;

        let mut loop_counter: u32 = 0;
        let mut tick_ms: u32 = 0;

        loop {
            if usb_dev.poll(&mut [&mut uf2_class]) {
                let mut chunk = [0u8; 64];
                if let Ok(len) = uf2_class.read_packet(&mut chunk) {
                    if len > 0 {
                        let avail = (512 - block_off).min(len);
                        block_buf[block_off..block_off + avail].copy_from_slice(&chunk[..avail]);
                        block_off += avail;

                        if block_off == 512 {
                            if let Some(uf2) = Uf2Block::parse(&block_buf) {
                                let _ = flash_engine.handle_uf2_block(uf2);
                                let _ = uf2_class.write_packet(&[0x00]); // ACK
                            }
                            block_off = 0;
                        }
                    }
                }
            }

            cortex_m::asm::delay(64);
            loop_counter = loop_counter.wrapping_add(1);

            // Feed watchdog if active from main application
            unsafe {
                core::ptr::write_volatile(0x4001_0600 as *mut u32, 0x6E52_4635);
            }

            if loop_counter % 1000 == 0 {
                tick_ms = tick_ms.wrapping_add(1);
                indicator.tick_nonblocking(tick_ms, reason);
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
        core::ptr::write_volatile(SCB_VTOR, image_start);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        cortex_m::asm::bootload(image_start as *const u32)
    }
}
