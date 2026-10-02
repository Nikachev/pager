//! Board startup settings and public chip identity.
use crate::RESET_REASON;
use core::sync::atomic::Ordering;
use embassy_nrf::{interrupt::Priority, pac};
use nrf_sdc::mpsl;

pub fn init() -> embassy_nrf::Peripherals {
    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = Priority::P2;
    config.time_interrupt_priority = Priority::P2;
    embassy_nrf::init(config)
}
pub fn record_reset_reason() {
    let reset_reason = pac::POWER.resetreas().read().0 | pac::POWER.gpregret2().read().0;
    pac::POWER
        .gpregret2()
        .write_value(pac::power::regs::Gpregret2(0));
    RESET_REASON.store(reset_reason, Ordering::Relaxed);
    pac::POWER
        .resetreas()
        .write_value(pac::power::regs::Resetreas(reset_reason));
    crate::log_msg!(
        "BOOT:VERSION:{}:RESET_REASON:{:x}",
        env!("PAGER_BUILD_VERSION"),
        reset_reason
    );
    crate::start_watchdog();
}
pub fn lfclk_config() -> mpsl::raw::mpsl_clock_lfclk_cfg_t {
    mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
        rc_ctiv: 0,
        rc_temp_ctiv: 0,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    }
}
pub fn factory_usb_serial() -> heapless::String<16> {
    let low = pac::FICR.deviceid(0).read();
    let high = pac::FICR.deviceid(1).read();
    let mut serial = heapless::String::new();
    let _ = core::fmt::write(&mut serial, format_args!("{:08x}{:08x}", high, low));
    serial
}
