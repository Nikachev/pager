#![no_std]

pub const MAX_TRACKED_BLOCKS: usize = 4096;
const BITMAP_WORDS: usize = MAX_TRACKED_BLOCKS / 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackError {
    InvalidCount,
    InconsistentCount,
    InvalidAddress,
    InvalidPayload,
    ConflictingLastPayload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    New,
    Duplicate,
}

pub struct UpdateTracker {
    received: [u32; BITMAP_WORDS],
    unique: u32,
    total: Option<u32>,
    last_payload_len: Option<u16>,
}

impl UpdateTracker {
    pub const fn new() -> Self {
        Self {
            received: [0; BITMAP_WORDS],
            unique: 0,
            total: None,
            last_payload_len: None,
        }
    }

    pub fn inspect(
        &mut self,
        block_no: u32,
        total: u32,
        target_addr: u32,
        payload_len: usize,
        firmware_start: u32,
        firmware_end: u32,
        payload_size: u32,
    ) -> Result<Observation, TrackError> {
        if total == 0 || total as usize > MAX_TRACKED_BLOCKS || block_no >= total {
            return Err(TrackError::InvalidCount);
        }
        if self.total.is_some_and(|previous| previous != total) {
            return Err(TrackError::InconsistentCount);
        }
        let expected_addr = block_no
            .checked_mul(payload_size)
            .and_then(|offset| firmware_start.checked_add(offset))
            .filter(|addr| *addr < firmware_end)
            .ok_or(TrackError::InvalidAddress)?;
        if target_addr != expected_addr {
            return Err(TrackError::InvalidAddress);
        }
        if payload_len == 0
            || payload_len > payload_size as usize
            || payload_len % 4 != 0
            || (block_no + 1 != total && payload_len != payload_size as usize)
            || target_addr
                .checked_add(payload_len as u32)
                .is_none_or(|end| end > firmware_end)
        {
            return Err(TrackError::InvalidPayload);
        }
        self.total = Some(total);
        if block_no + 1 == total {
            let len = payload_len as u16;
            if self
                .last_payload_len
                .is_some_and(|previous| previous != len)
            {
                return Err(TrackError::ConflictingLastPayload);
            }
            self.last_payload_len = Some(len);
        }
        let index = block_no as usize;
        if self.contains(index) {
            return Ok(Observation::Duplicate);
        }
        Ok(Observation::New)
    }

    pub fn commit(&mut self, block_no: u32) {
        let index = block_no as usize;
        if !self.contains(index) {
            self.received[index / 32] |= 1 << (index % 32);
            self.unique += 1;
        }
    }

    pub fn complete(&self) -> bool {
        self.total.is_some_and(|total| self.unique == total)
    }

    pub fn total(&self) -> Option<u32> {
        self.total
    }

    pub fn last_payload_len(&self) -> Option<u16> {
        self.last_payload_len
    }

    fn contains(&self, index: usize) -> bool {
        self.received[index / 32] & (1 << (index % 32)) != 0
    }
}

impl Default for UpdateTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: u32 = 0xC000;
    const END: u32 = 0xFE000;

    #[test]
    fn accepts_out_of_order_and_exact_duplicates() {
        let mut tracker = UpdateTracker::new();
        assert_eq!(
            tracker.inspect(2, 3, START + 512, 128, START, END, 256),
            Ok(Observation::New)
        );
        tracker.commit(2);
        assert_eq!(
            tracker.inspect(0, 3, START, 256, START, END, 256),
            Ok(Observation::New)
        );
        tracker.commit(0);
        assert_eq!(
            tracker.inspect(2, 3, START + 512, 128, START, END, 256),
            Ok(Observation::Duplicate)
        );
        assert!(!tracker.complete());
        assert_eq!(
            tracker.inspect(1, 3, START + 256, 256, START, END, 256),
            Ok(Observation::New)
        );
        tracker.commit(1);
        assert!(tracker.complete());
    }

    #[test]
    fn rejects_inconsistent_counts_addresses_and_lengths() {
        let mut tracker = UpdateTracker::new();
        assert_eq!(
            tracker.inspect(0, 2, START, 256, START, END, 256),
            Ok(Observation::New)
        );
        tracker.commit(0);
        assert_eq!(
            tracker.inspect(1, 3, START + 256, 256, START, END, 256),
            Err(TrackError::InconsistentCount)
        );
        assert_eq!(
            tracker.inspect(1, 2, START + 512, 256, START, END, 256),
            Err(TrackError::InvalidAddress)
        );
        assert_eq!(
            tracker.inspect(1, 2, START + 256, 3, START, END, 256),
            Err(TrackError::InvalidPayload)
        );
    }

    #[test]
    fn checked_arithmetic_rejects_overflow() {
        let mut tracker = UpdateTracker::new();
        assert_eq!(
            tracker.inspect(u32::MAX, u32::MAX, START, 256, START, END, 256),
            Err(TrackError::InvalidCount)
        );
    }

    #[test]
    fn every_rotated_block_order_completes_exactly_once() {
        const TOTAL: u32 = 32;
        for rotation in 0..TOTAL {
            let mut tracker = UpdateTracker::new();
            for step in 0..TOTAL {
                let block = (step + rotation) % TOTAL;
                let len = if block + 1 == TOTAL { 128 } else { 256 };
                assert_eq!(
                    tracker.inspect(block, TOTAL, START + block * 256, len, START, END, 256,),
                    Ok(Observation::New)
                );
                tracker.commit(block);
                assert_eq!(
                    tracker.inspect(block, TOTAL, START + block * 256, len, START, END, 256,),
                    Ok(Observation::Duplicate)
                );
            }
            assert!(tracker.complete());
            assert_eq!(tracker.last_payload_len(), Some(128));
        }
    }
}
