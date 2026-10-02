//! One owned text job, advanced one report at a time by the BLE event loop.
use crate::ble::ascii_to_hid;

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Report([u8; 8]),
    Complete(u8),
}

pub struct Job {
    pub id: u32,
    text: heapless::String<256>,
    offset: usize,
    deadline: u64,
    due: u64,
    interval: u64,
    held: bool,
    terminal: Option<u8>,
}
impl Job {
    pub fn new(
        id: u32,
        text: heapless::String<256>,
        deadline: u64,
        now: u64,
        interval: u64,
    ) -> Self {
        Self {
            id,
            text,
            offset: 0,
            deadline,
            due: now,
            interval,
            held: false,
            terminal: None,
        }
    }
    pub fn due(&self) -> u64 {
        self.due.min(self.deadline)
    }
    pub fn report_sent(&mut self, now: u64) {
        self.due = now.saturating_add(self.interval);
    }
    pub fn needs_release(&self) -> bool {
        self.held
    }
    pub fn advance(&mut self, now: u64) -> Step {
        if self.held {
            self.held = false;
            self.due = now.saturating_add(self.interval);
            if now >= self.deadline {
                self.terminal = Some(1);
            }
            return Step::Report([0; 8]);
        }
        if let Some(result) = self.terminal {
            return Step::Complete(result);
        }
        if now >= self.deadline {
            return Step::Complete(1);
        }
        let Some(ch) = self.text[self.offset..].chars().next() else {
            return Step::Complete(0);
        };
        let Some((modifier, keycode)) = ascii_to_hid(ch) else {
            return Step::Complete(2);
        };
        self.offset += ch.len_utf8();
        self.held = true;
        self.due = now.saturating_add(self.interval);
        Step::Report([modifier, 0, keycode, 0, 0, 0, 0, 0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(text: &str, deadline: u64) -> Job {
        Job::new(42, text.try_into().unwrap(), deadline, 0, 12)
    }
    #[test]
    fn reports_preserve_press_release_spacing_and_finish_after_release() {
        let mut j = job("ab", 100);
        assert_eq!(j.advance(0), Step::Report([0, 0, 4, 0, 0, 0, 0, 0]));
        assert_eq!(j.due(), 12);
        assert!(j.needs_release());
        assert_eq!(j.advance(12), Step::Report([0; 8]));
        assert_eq!(j.due(), 24);
        assert_eq!(j.advance(24), Step::Report([0, 0, 5, 0, 0, 0, 0, 0]));
        assert_eq!(j.advance(36), Step::Report([0; 8]));
        assert_eq!(j.advance(48), Step::Complete(0));
    }
    #[test]
    fn notifier_delay_preserves_spacing_and_cancellation_needs_release() {
        let mut j = job("ab", 1000);
        j.advance(0);
        j.report_sent(30);
        assert_eq!(j.due(), 42);
        assert!(j.needs_release());
        j.advance(42);
        j.report_sent(60);
        assert_eq!(j.due(), 72);
        assert!(!j.needs_release());
    }
    #[test]
    fn expiry_releases_held_key_and_never_starts_next_character() {
        let mut j = job("ab", 5);
        j.advance(0);
        assert_eq!(j.due(), 5);
        assert_eq!(j.advance(5), Step::Report([0; 8]));
        assert_eq!(j.advance(5), Step::Complete(1));
        let mut expired = job("a", 0);
        assert_eq!(expired.advance(0), Step::Complete(1));
    }
    #[test]
    fn unsupported_unicode_and_empty_text_complete_without_press() {
        assert_eq!(job("ж", 100).advance(0), Step::Complete(2));
        assert_eq!(job("", 100).advance(0), Step::Complete(0));
    }
}
