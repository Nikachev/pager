//! Helper utilities for USB detach and system reboot.

use embassy_time::{Duration, Timer};

/// NRF52840 USBD peripheral base address and control registers.
const USBD_BASE: usize = 0x4002_7000;
const USBD_ENABLE_REG: *mut u32 = (USBD_BASE + 0x500) as *mut u32;
const USBD_USBPULLUP_REG: *mut u32 = (USBD_BASE + 0x504) as *mut u32;

/// Detach USB pullups so host OS disconnects USB cleanly before entering bootloader.
pub async fn reset_after_usb_detach() -> ! {
    unsafe {
        core::ptr::write_volatile(USBD_USBPULLUP_REG, 0);
        core::ptr::write_volatile(USBD_ENABLE_REG, 0);
    }
    Timer::after(Duration::from_millis(500)).await;
    crate::flash::enter_bootloader();
}
