//! Fixed-size public diagnostics. Bond/key material is never accepted here.
use core::sync::atomic::{AtomicU32, Ordering};

pub fn increment_saturated(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

pub struct Snapshot {
    pub reset: u32,
    pub logs_drop: u32,
    pub history_drop: u32,
    pub log_high: u32,
    pub cmd_drop: u32,
    pub cmd_high: u32,
    pub storage_error: u32,
    pub uptime_ms: u64,
}

impl Snapshot {
    pub fn format(&self) -> heapless::String<256> {
        let mut line = heapless::String::new();
        core::fmt::write(&mut line, format_args!(
            "DIAG:reset={:x};logs_drop={};history_drop={};log_high={};cmd_drop={};cmd_high={};storage_error={};uptime_ms={}\n",
            self.reset, self.logs_drop, self.history_drop, self.log_high, self.cmd_drop,
            self.cmd_high, self.storage_error, self.uptime_ms,
        )).expect("maximum diagnostic integers fit the fixed buffer");
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_saturate_instead_of_wrapping_to_zero() {
        let counter = AtomicU32::new(u32::MAX - 1);
        increment_saturated(&counter);
        increment_saturated(&counter);
        assert_eq!(counter.load(Ordering::Relaxed), u32::MAX);
    }
    #[test]
    fn maximum_snapshot_fits_and_keeps_live_uptime() {
        let s = Snapshot {
            reset: u32::MAX,
            logs_drop: u32::MAX,
            history_drop: u32::MAX,
            log_high: u32::MAX,
            cmd_drop: u32::MAX,
            cmd_high: u32::MAX,
            storage_error: u32::MAX,
            uptime_ms: u64::MAX,
        };
        let line = s.format();
        assert!(line.ends_with("uptime_ms=18446744073709551615\n"));
        assert!(line.len() < 256);
    }
}
