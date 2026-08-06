//! Bootloader Status Indicator LED
//!
//! Morse/Pulse patterns with prefix [3 short blinks (···)]:
//! - User Request (Double-tap/Soft): [3 short] + Pause
//! - Integrity Error (Digest):      [3 short] + 1 Long + Pause
//! - Signature Error (Ed25519):     [3 short] + 2 Long + Pause
//! - Blank / No Firmware:           [3 short] + 3 Long + Pause

use embassy_nrf::gpio::Output;
use embassy_time::Timer;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BootReason {
    UserRequest,
    IntegrityError,
    SignatureError,
    NoFirmware,
}

pub struct LedIndicator<'a> {
    pin: Output<'a>,
}

impl<'a> LedIndicator<'a> {
    pub fn new(pin: Output<'a>) -> Self {
        Self { pin }
    }

    pub fn blink_pattern_sync(&mut self, reason: BootReason) {
        // Active-low LED logic on nRF52840 (Low = On, High = Off)
        // 64MHz ARM Cortex-M4: 64,000 cycles per millisecond
        let ms_cycles = 64_000;

        // 1. Prefix: 3 short, crisp blinks (60ms ON / 80ms OFF)
        for _ in 0..3 {
            self.pin.set_low();
            cortex_m::asm::delay(60 * ms_cycles);
            self.pin.set_high();
            cortex_m::asm::delay(80 * ms_cycles);
        }

        // Inter-sequence pause (120ms)
        cortex_m::asm::delay(120 * ms_cycles);

        // 2. Reason code in long blinks (250ms ON / 150ms OFF)
        let long_blinks = match reason {
            BootReason::UserRequest => 0,
            BootReason::IntegrityError => 1,
            BootReason::SignatureError => 2,
            BootReason::NoFirmware => 3,
        };

        for _ in 0..long_blinks {
            self.pin.set_low();
            cortex_m::asm::delay(250 * ms_cycles);
            self.pin.set_high();
            cortex_m::asm::delay(150 * ms_cycles);
        }

        // End of sequence cycle pause (600ms)
        cortex_m::asm::delay(600 * ms_cycles);
    }

    pub fn tick_nonblocking(&mut self, tick_ms: u32, reason: BootReason) {
        let long_blinks = match reason {
            BootReason::UserRequest => 0,
            BootReason::IntegrityError => 1,
            BootReason::SignatureError => 2,
            BootReason::NoFirmware => 3,
        };

        let short_phase = 3 * 140; // 420ms
        let pause1 = 120; // 540ms
        let long_phase = long_blinks * 400;
        let total_period = 540 + long_phase + 600;

        let t = tick_ms % total_period;

        if t < short_phase {
            let sub = t % 140;
            if sub < 60 {
                self.pin.set_low();
            } else {
                self.pin.set_high();
            }
        } else if t < short_phase + pause1 {
            self.pin.set_high();
        } else if t < short_phase + pause1 + long_phase {
            let sub = (t - (short_phase + pause1)) % 400;
            if sub < 250 {
                self.pin.set_low();
            } else {
                self.pin.set_high();
            }
        } else {
            self.pin.set_high();
        }
    }

    pub async fn blink_pattern(&mut self, reason: BootReason) {
        // Active-low LED logic on nRF52840 (Low = On, High = Off)
        for _ in 0..3 {
            self.pin.set_low();
            Timer::after_millis(60).await;
            self.pin.set_high();
            Timer::after_millis(80).await;
        }

        Timer::after_millis(120).await;

        let long_blinks = match reason {
            BootReason::UserRequest => 0,
            BootReason::IntegrityError => 1,
            BootReason::SignatureError => 2,
            BootReason::NoFirmware => 3,
        };

        for _ in 0..long_blinks {
            self.pin.set_low();
            Timer::after_millis(250).await;
            self.pin.set_high();
            Timer::after_millis(150).await;
        }

        Timer::after_millis(600).await;
    }
}
