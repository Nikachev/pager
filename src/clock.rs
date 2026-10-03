//! GPS-disciplined UTC milliseconds, advanced by a monotonic board clock.
//! Until a valid GPS date/time arrives, UTC is unknown. Loss of GPS uses holdover.
#[derive(Default)]
pub struct Clock {
    anchor: Option<(u64, u64)>,
}
impl Clock {
    pub const fn new() -> Self {
        Self { anchor: None }
    }
    pub fn correct(&mut self, unix_ms: u64, monotonic_ms: u64) {
        self.anchor = Some((unix_ms, monotonic_ms));
    }
    pub fn now(&self, monotonic_ms: u64) -> Option<u64> {
        let (unix, anchor) = self.anchor?;
        unix.checked_add(monotonic_ms.checked_sub(anchor)?)
    }
    pub fn sync_age_ms(&self, monotonic_ms: u64) -> Option<u64> {
        monotonic_ms.checked_sub(self.anchor?.1)
    }
}

/// NMEA RMC ddmmyy + hhmmss[.fraction], UTC. Reject impossible dates/times.
pub fn nmea_unix_ms(date: &str, time: &str) -> Option<u64> {
    fn pair(s: &str) -> Option<u32> {
        if s.len() != 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }
    if date.len() != 6 || !date.is_ascii() || time.len() < 6 || !time.is_ascii() {
        return None;
    }
    let day = pair(&date[0..2])?;
    let month = pair(&date[2..4])?;
    let year = pair(&date[4..6])?;
    let year = if year >= 80 { 1900 + year } else { 2000 + year };
    let hour = pair(&time[0..2])?;
    let minute = pair(&time[2..4])?;
    let second = pair(&time[4..6])?;
    if hour > 23 || minute > 59 || second > 59 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = |y: u32| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let days_in_month = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > days_in_month[(month - 1) as usize] {
        return None;
    }
    let mut ms = 0;
    if time.len() > 6 {
        let fractional = time.strip_prefix(&time[..6])?.strip_prefix('.')?;
        if fractional.is_empty() || !fractional.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        for (i, b) in fractional.bytes().take(3).enumerate() {
            ms += u64::from(b - b'0') * [100, 10, 1][i];
        }
    }
    let mut days = 0u64;
    for y in 1970..year {
        days += if leap(y) { 366 } else { 365 };
    }
    days += days_in_month[..(month - 1) as usize]
        .iter()
        .map(|d| u64::from(*d))
        .sum::<u64>()
        + u64::from(day - 1);
    Some((days * 86400 + u64::from(hour * 3600 + minute * 60 + second)) * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_calendar_and_fraction() {
        assert_eq!(nmea_unix_ms("010180", "000000"), Some(315532800000));
        assert_eq!(nmea_unix_ms("031026", "123456.7899"), Some(1791030896789));
        assert!(nmea_unix_ms("290224", "235959.1").is_some());
        for (date, time) in [
            ("290223", "120000"),
            ("310426", "120000"),
            ("001026", "120000"),
            ("011326", "120000"),
            ("031026", "240000"),
            ("031026", "125960"),
            ("031026", "123456."),
            ("031026", "123456x"),
        ] {
            assert_eq!(nmea_unix_ms(date, time), None);
        }
    }
    #[test]
    fn unknown_correction_holdover_and_midnight() {
        let mut c = Clock::default();
        assert_eq!(c.now(500), None);
        c.correct(nmea_unix_ms("311225", "235959").unwrap(), 100);
        assert_eq!(c.now(1100), nmea_unix_ms("010126", "000000"));
        assert_eq!(c.sync_age_ms(11100), Some(11000));
        assert_eq!(c.now(99), None);
        c.correct(100000, 11100);
        assert_eq!(c.now(12100), Some(101000));
    }
}
