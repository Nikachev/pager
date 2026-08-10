//! Bootloader Status Indicator LED
//!
//! Morse/Pulse patterns with prefix [3 short blinks (···)]:
//! - User Request (Double-tap/Soft): [3 short] + Pause
//! - Integrity Error (Digest):      [3 short] + 1 Long + Pause
//! - Signature Error (Ed25519):     [3 short] + 2 Long + Pause
//! - Blank / No Firmware:           [3 short] + 3 Long + Pause

use embassy_nrf::gpio::Output;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BootReason {
    UserRequest,
    IntegrityError,
    SignatureError,
    NoFirmware,
    Fault,
}

pub struct LedIndicator<'a> {
    pin: Output<'a>,
}

impl<'a> LedIndicator<'a> {
    pub fn new(pin: Output<'a>) -> Self {
        Self { pin }
    }

    pub fn tick_nonblocking(&mut self, tick_ms: u32, reason: BootReason) {
        let long_blinks = match reason {
            BootReason::UserRequest => 0,
            BootReason::IntegrityError => 1,
            BootReason::SignatureError => 2,
            BootReason::NoFirmware => 3,
            BootReason::Fault => 4,
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

}
