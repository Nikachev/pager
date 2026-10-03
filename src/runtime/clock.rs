//! Shared board UTC source. Call `now_utc_ms` from any task; never use host time.
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{raw::ThreadModeRawMutex, Mutex};
use embassy_time::Instant;
static CLOCK: Mutex<ThreadModeRawMutex, RefCell<pager::clock::Clock>> =
    Mutex::new(RefCell::new(pager::clock::Clock::new()));

pub fn now_utc_ms() -> Option<u64> {
    CLOCK.lock(|c| c.borrow().now(Instant::now().as_millis()))
}
pub fn sync_age_ms() -> Option<u64> {
    CLOCK.lock(|c| c.borrow().sync_age_ms(Instant::now().as_millis()))
}
#[cfg(feature = "board-xiao-nrf52840")]
pub fn correct_from_gps(unix_ms: u64) {
    CLOCK.lock(|c| c.borrow_mut().correct(unix_ms, Instant::now().as_millis()));
}
