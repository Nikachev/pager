//! Opt-in development-only, one-shot hardware fault injection.
//! Standard builds have no reachable injection control plane.
use core::sync::atomic::{AtomicU32, Ordering};
static ARMED: AtomicU32 = AtomicU32::new(0);
pub const ENABLED: bool = option_env!("PAGER_FAULT_INJECTION").is_some();

pub fn arm(action: u8) -> bool {
    ENABLED
        && (1..=7).contains(&action)
        && ARMED
            .compare_exchange(0, action as u32, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}
pub fn take(action: u8) -> bool {
    ENABLED
        && ARMED
            .compare_exchange(action as u32, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn standard_build_cannot_arm_faults() {
        assert!(!super::ENABLED);
        for action in 0..=255 {
            assert!(!super::arm(action));
            assert!(!super::take(action));
        }
    }
}
