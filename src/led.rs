//! LED status animation task and signaling.

use embassy_futures::select::Either;
use embassy_nrf::gpio::Output;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

pub static LED_MODE: Signal<ThreadModeRawMutex, u8> = Signal::new();

#[embassy_executor::task]
pub async fn blink_task(mut led: Output<'static>) -> ! {
    let mut mode = 0;
    loop {
        crate::signal_heartbeat(crate::HEARTBEAT_BLINK);
        match mode {
            0 => {
                led.set_low(); // ON
                if let Either::Second(new_mode) = embassy_futures::select::select(
                    Timer::after(Duration::from_millis(50)),
                    LED_MODE.wait(),
                )
                .await
                {
                    mode = new_mode;
                    continue;
                }
                led.set_high(); // OFF
                if let Either::Second(new_mode) = embassy_futures::select::select(
                    Timer::after(Duration::from_millis(1950)),
                    LED_MODE.wait(),
                )
                .await
                {
                    mode = new_mode;
                }
            }
            1 => {
                led.set_high(); // OFF
                mode = LED_MODE.wait().await;
            }
            2 => {
                led.set_low(); // ON
                mode = LED_MODE.wait().await;
            }
            _ => {
                mode = 0;
            }
        }
    }
}
