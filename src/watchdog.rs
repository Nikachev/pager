//! Progress model shared by the hardware watchdog and host liveness tests.
#[derive(Default)]
pub struct ProgressWatchdog {
    age: [u8; 4],
}

impl ProgressWatchdog {
    /// Called every two seconds. Idle waits must explicitly report progress;
    /// active work has a 14-second allowance (control deadline is 10 seconds).
    pub fn observe(&mut self, required: u32, seen: u32) -> bool {
        for (index, age) in self.age.iter_mut().enumerate() {
            let bit = 1 << index;
            *age = if required & bit == 0 || seen & bit != 0 {
                0
            } else {
                age.saturating_add(1)
            };
        }
        self.age.iter().all(|age| *age < 7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_progress_and_legal_work_do_not_trigger_reset() {
        let mut w = ProgressWatchdog::default();
        for _ in 0..100 {
            assert!(w.observe(15, 15));
        }
        for _ in 0..5 {
            assert!(w.observe(15, 1));
        }
        assert!(w.observe(15, 15));
        for _ in 0..5 {
            assert!(w.observe(15, 1));
        }
    }
    #[test]
    fn blink_cannot_hide_another_task_stall() {
        for missing in [1, 2, 4, 8] {
            let mut w = ProgressWatchdog::default();
            for _ in 0..6 {
                assert!(w.observe(15, 15 ^ missing));
            }
            assert!(!w.observe(15, 15 ^ missing));
            assert!(w.observe(15, 15));
        }
    }
    #[test]
    fn unstarted_tasks_are_not_required() {
        let mut w = ProgressWatchdog::default();
        for _ in 0..100 {
            assert!(w.observe(1, 1));
        }
    }
}
