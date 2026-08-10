//! Double-Tap Reset & Retention Register Logic
//!
//! Uses GPREGRET (8-bit General Purpose Retention Register: 0x4000_051C, bits [7:0] only).
//! Detects software DFU request and double-tap reset within 500ms window.

/// 8-bit magic value for DFU entry request (written by firmware's enter_bootloader())
const DFU_MAGIC: u8 = 0xB1;
/// 8-bit magic for double-tap window detection
const DBL_TAP_MAGIC: u8 = 0xA5;
const FAULT_MAGIC: u8 = 0xE1;
const NRF_POWER_GPREGRET: *mut u32 = 0x4000_051C as *mut u32;
const NRF_POWER_RESETREAS: *mut u32 = 0x4000_0400 as *mut u32;

pub fn check_and_set_double_tap() -> bool {
    // GPREGRET is 8-bit — only bits [7:0] are retained across reset
    let val = (unsafe { core::ptr::read_volatile(NRF_POWER_GPREGRET) } & 0xFF) as u8;

    if val == DFU_MAGIC || val == DBL_TAP_MAGIC {
        unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
        true
    } else {
        let resetreas = unsafe { core::ptr::read_volatile(NRF_POWER_RESETREAS) };
        if resetreas != 0 {
            unsafe { core::ptr::write_volatile(NRF_POWER_RESETREAS, resetreas) };
        }

        // Only enter 500ms window if reset was triggered by physical RESET pin (bit 0)
        let is_pin_reset = (resetreas & 0x01) != 0;

        if is_pin_reset {
            unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, DBL_TAP_MAGIC as u32) };
            cortex_m::asm::delay(500 * 64_000); // 500ms window
            unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
        } else {
            unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
        }
        false
    }
}

pub fn clear_double_tap() {
    unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
}

pub fn mark_fault() {
    unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, FAULT_MAGIC as u32) };
}

pub fn take_fault() -> bool {
    let value = unsafe { core::ptr::read_volatile(NRF_POWER_GPREGRET) } as u8;
    if value == FAULT_MAGIC {
        unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
        true
    } else {
        false
    }
}
