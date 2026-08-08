//! Helper utilities for USB detach and system reboot.

use embassy_nrf::pac;
use embassy_time::{Duration, Timer};

/// Detach USB pullups so host OS disconnects USB cleanly before entering bootloader.
pub async fn reset_after_usb_detach() -> ! {
    pac::USBD
        .usbpullup()
        .write_value(pac::usbd::regs::Usbpullup(0));
    pac::USBD.enable().write_value(pac::usbd::regs::Enable(0));
    Timer::after(Duration::from_millis(500)).await;
    crate::flash::enter_bootloader();
}
