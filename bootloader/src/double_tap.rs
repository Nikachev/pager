//! Double-Tap Reset & Retention Register Logic
//!
//! Uses GPREGRET (8-bit General Purpose Retention Register 0: 0x4000_051C) on nRF52840 POWER peripheral.
//! Holds value across software & Pin resets (cleared only on Power-On Reset).

const DBL_TAP_MAGIC: u8 = 0xA5;
const NRF_POWER_GPREGRET: *mut u8 = 0x4000_051C as *mut u8;
const NRF_POWER_RESETREAS: *mut u32 = 0x4000_0400 as *mut u32;

pub fn check_and_set_double_tap() -> bool {
    let resetreas = unsafe { core::ptr::read_volatile(NRF_POWER_RESETREAS) };
    let val = unsafe { core::ptr::read_volatile(NRF_POWER_GPREGRET) };

    // Clear RESETREAS by writing 1s to set bits
    unsafe { core::ptr::write_volatile(NRF_POWER_RESETREAS, resetreas) };

    if val == DBL_TAP_MAGIC {
        // Double tap or software reboot request: DFU confirmed!
        unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
        true
    } else {
        // Fast 15ms debounce so even quick tweezer taps record the magic
        cortex_m::asm::delay(15 * 64_000);
        unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, DBL_TAP_MAGIC) };
        false
    }
}

pub fn clear_double_tap() {
    unsafe { core::ptr::write_volatile(NRF_POWER_GPREGRET, 0) };
}
