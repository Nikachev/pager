//! One-shot factory-UF2 installer for the Pager bootloader on a stock XIAO nRF52840.
//!
//! This image runs at the S140 v7 application address. It deliberately removes
//! the factory MBR/SoftDevice and installs the Pager bootloader at address zero.

#![no_main]
#![no_std]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;

const PAGE_SIZE: u32 = 4096;
const PAGER_BOOTLOADER_END: u32 = 0x0000_C000;
const FACTORY_APPLICATION_START: u32 = 0x0002_7000;

const NVMC_BASE: u32 = 0x4001_E000;
const NVMC_READY: *const u32 = (NVMC_BASE + 0x400) as *const u32;
const NVMC_CONFIG: *mut u32 = (NVMC_BASE + 0x504) as *mut u32;
const NVMC_ERASEPAGE: *mut u32 = (NVMC_BASE + 0x508) as *mut u32;
const NVMC_REN: u32 = 0;
const NVMC_WEN: u32 = 1;
const NVMC_EEN: u32 = 2;
const ACL_REGIONS: u32 = 8;
const ACL_REGION_BASE: u32 = NVMC_BASE + 0x800;
const ACL_REGION_STRIDE: u32 = 0x10;

const GPIO_P0_BASE: u32 = 0x5000_0000;
const GPIO_OUTSET: *mut u32 = (GPIO_P0_BASE + 0x508) as *mut u32;
const GPIO_OUTCLR: *mut u32 = (GPIO_P0_BASE + 0x50C) as *mut u32;
const GPIO_DIRSET: *mut u32 = (GPIO_P0_BASE + 0x518) as *mut u32;
const BLUE_LED: u32 = 1 << 26;

const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const WDT_BASE: u32 = 0x4001_0000;
const WDT_RUNSTATUS: *const u32 = (WDT_BASE + 0x400) as *const u32;
const WDT_RREN: *const u32 = (WDT_BASE + 0x508) as *const u32;
const WDT_RR0: u32 = WDT_BASE + 0x600;
const WDT_RELOAD: u32 = 0x6E52_4635;

static BOOTLOADER: &[u8] = include_bytes!(env!("PAGER_INSTALLER_BOOTLOADER_BIN"));

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    error_blink()
}

#[entry]
fn main() -> ! {
    unsafe {
        cortex_m::interrupt::disable();
        write_volatile(SYST_CSR, 0);
        for index in 0..8u32 {
            write_volatile((0xE000_E180 + index * 4) as *mut u32, u32::MAX);
            write_volatile((0xE000_E280 + index * 4) as *mut u32, u32::MAX);
        }
        led_init();
        led_on();
    }

    // ACL permissions survive the factory bootloader's jump to this application
    // and cannot be cleared here. Refuse before touching MBR or SoftDevice.
    if !flash_is_accessible() || !valid_embedded_bootloader() {
        error_blink();
    }
    feed_watchdog();

    // Keep page zero (the factory MBR reset vector) intact until every other
    // Pager bootloader page has been written and verified.
    let mut page = PAGE_SIZE;
    while page < PAGER_BOOTLOADER_END {
        erase_page(page);
        program_page(page);
        if !verify_page(page) {
            error_blink();
        }
        feed_watchdog();
        page += PAGE_SIZE;
    }

    // Remove the remainder of S140. This also guarantees that the Pager
    // application manifest at 0xC000 is blank on the first custom boot.
    while page < FACTORY_APPLICATION_START {
        erase_page(page);
        feed_watchdog();
        page += PAGE_SIZE;
    }

    erase_page(0);
    program_page(0);
    if !verify_page(0) || !verify_bootloader() {
        error_blink();
    }

    unsafe {
        write_volatile(NVMC_CONFIG, NVMC_REN);
    }
    wait_ready();
    feed_watchdog();
    cortex_m::peripheral::SCB::sys_reset()
}

fn valid_embedded_bootloader() -> bool {
    if BOOTLOADER.len() < 8 || BOOTLOADER.len() > PAGER_BOOTLOADER_END as usize {
        return false;
    }
    let stack = u32::from_le_bytes(BOOTLOADER[0..4].try_into().unwrap());
    let reset = u32::from_le_bytes(BOOTLOADER[4..8].try_into().unwrap());
    (0x2000_0000..=0x2004_0000).contains(&stack)
        && reset & 1 == 1
        && (reset & !1) < BOOTLOADER.len() as u32
}

fn flash_is_accessible() -> bool {
    for index in 0..ACL_REGIONS {
        let region = ACL_REGION_BASE + index * ACL_REGION_STRIDE;
        let (address, size, permissions) = unsafe {
            (
                read_volatile(region as *const u32),
                read_volatile((region + 4) as *const u32),
                read_volatile((region + 8) as *const u32),
            )
        };
        if pager_xiao_installer::acl_blocks_migration(
            address,
            size,
            permissions,
            FACTORY_APPLICATION_START,
        ) {
            return false;
        }
    }
    true
}

fn erase_page(address: u32) {
    unsafe {
        write_volatile(NVMC_CONFIG, NVMC_EEN);
    }
    wait_ready();
    unsafe {
        write_volatile(NVMC_ERASEPAGE, address);
    }
    wait_ready();
}

fn program_page(address: u32) {
    unsafe {
        write_volatile(NVMC_CONFIG, NVMC_WEN);
    }
    wait_ready();

    let start = address as usize;
    if start >= BOOTLOADER.len() {
        return;
    }
    let end = (start + PAGE_SIZE as usize).min(BOOTLOADER.len());
    // On the final page-zero commit, write the initial stack pointer last. A
    // reset vector is therefore never exposed together with a half-written
    // vector page during normal execution.
    let mut offset = if address == 0 { 8 } else { start };
    while offset < end {
        program_word(offset, end);
        offset += 4;
    }
    if address == 0 {
        program_word(4, end);
        program_word(0, end);
    }
}

fn program_word(offset: usize, end: usize) {
    let mut bytes = [0xFF; 4];
    let count = (end - offset).min(4);
    bytes[..count].copy_from_slice(&BOOTLOADER[offset..offset + count]);
    unsafe {
        write_volatile(offset as *mut u32, u32::from_le_bytes(bytes));
    }
    wait_ready();
}

fn verify_page(address: u32) -> bool {
    let start = address as usize;
    let end = (start + PAGE_SIZE as usize).min(BOOTLOADER.len());
    if start >= end {
        return true;
    }
    for (offset, expected) in BOOTLOADER[start..end].iter().enumerate() {
        let actual = unsafe { read_volatile((start + offset) as *const u8) };
        if actual != *expected {
            return false;
        }
    }
    true
}

fn verify_bootloader() -> bool {
    BOOTLOADER
        .iter()
        .enumerate()
        .all(|(offset, expected)| (unsafe { read_volatile(offset as *const u8) }) == *expected)
}

fn wait_ready() {
    while unsafe { read_volatile(NVMC_READY) } == 0 {
        core::hint::spin_loop();
    }
}

fn feed_watchdog() {
    unsafe {
        if read_volatile(WDT_RUNSTATUS) == 0 {
            return;
        }
        let enabled = read_volatile(WDT_RREN);
        for register in 0..8u32 {
            if enabled & (1 << register) != 0 {
                write_volatile((WDT_RR0 + register * 4) as *mut u32, WDT_RELOAD);
            }
        }
    }
}

unsafe fn led_init() {
    write_volatile(GPIO_DIRSET, BLUE_LED);
    led_off();
}

unsafe fn led_on() {
    write_volatile(GPIO_OUTCLR, BLUE_LED);
}

unsafe fn led_off() {
    write_volatile(GPIO_OUTSET, BLUE_LED);
}

fn error_blink() -> ! {
    unsafe {
        led_init();
    }
    loop {
        unsafe { led_on() };
        cortex_m::asm::delay(6_400_000);
        unsafe { led_off() };
        cortex_m::asm::delay(6_400_000);
        feed_watchdog();
    }
}
