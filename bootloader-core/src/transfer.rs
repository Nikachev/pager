//! Retain multi-sector progress across nonblocking transport retries.
#[derive(Default)]
pub struct SectorCursor {
    command: Option<(u64, u64)>,
    sector: u64,
    offset: usize,
}

impl SectorCursor {
    pub fn begin(&mut self, lba: u64, count: u64) {
        if self.command != Some((lba, count)) {
            self.command = Some((lba, count));
            self.sector = 0;
            self.offset = 0;
        }
    }
    pub fn position(&self) -> Option<(u64, usize)> {
        let (lba, count) = self.command?;
        if self.sector >= count {
            return None;
        }
        Some((lba.checked_add(self.sector)?, self.offset))
    }
    pub fn advance(&mut self, bytes: usize) -> bool {
        if self.position().is_none() || bytes > 512 - self.offset {
            return false;
        }
        self.offset += bytes;
        if self.offset == 512 {
            self.offset = 0;
            self.sector += 1;
        }
        true
    }
    pub fn complete(&self) -> bool {
        self.command.is_some_and(|(_, count)| self.sector == count)
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_preserve_sector_and_byte_position() {
        let mut cursor = SectorCursor::default();
        cursor.begin(99, 3);
        for sector in 0..3 {
            for offset in (0..512).step_by(64) {
                cursor.begin(99, 3);
                assert_eq!(cursor.position(), Some((99 + sector, offset)));
                assert!(cursor.advance(64));
                assert!(cursor.advance(0) || cursor.complete());
            }
        }
        assert!(cursor.complete());
        cursor.reset();
        cursor.begin(99, 3);
        assert_eq!(cursor.position(), Some((99, 0)));
        assert!(!cursor.advance(513));
        cursor.begin(100, 1);
        assert_eq!(cursor.position(), Some((100, 0)));
    }
    #[test]
    fn zero_and_overflow_do_not_wrap_lba() {
        let mut cursor = SectorCursor::default();
        cursor.begin(0, 0);
        assert!(cursor.complete());
        cursor.begin(u64::MAX, 2);
        assert!(cursor.advance(512));
        assert_eq!(cursor.position(), None);
        assert!(!cursor.complete());
    }
}
