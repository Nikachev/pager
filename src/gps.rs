//! Bounded streaming NMEA receiver. Only checksum-validated GGA updates fix data.
#[derive(Default)]
pub struct Receiver {
    line: heapless::Vec<u8, 128>,
    pub bytes: u32,
    pub position: Option<(i32, i32)>,
    /// Set only by the byte completing a checksum-valid, active RMC with a valid date.
    pub utc_update_ms: Option<u64>,
    /// True only when the most recently fed byte completed a valid GGA.
    pub gga_updated: bool,
    pub valid: u32,
    pub invalid: u32,
    pub quality: u8,
    pub satellites: u8,
    pub gga: heapless::String<128>,
    pub last_sentence: heapless::String<128>,
}

impl Receiver {
    pub fn feed(&mut self, byte: u8) -> bool {
        self.gga_updated = false;
        self.utc_update_ms = None;
        self.bytes = self.bytes.saturating_add(1);
        if byte == b'$' {
            self.line.clear();
        } else if self.line.is_empty() {
            return false;
        }
        if byte == b'\r' {
            return false;
        }
        if byte != b'\n' {
            if self.line.push(byte).is_err() {
                self.invalid = self.invalid.saturating_add(1);
                self.line.clear();
            }
            return false;
        }
        let accepted = self.parse();
        self.line.clear();
        if accepted {
            self.valid = self.valid.saturating_add(1);
        } else {
            self.invalid = self.invalid.saturating_add(1);
        }
        accepted
    }

    fn parse(&mut self) -> bool {
        let Ok(line) = core::str::from_utf8(&self.line) else {
            return false;
        };
        let Some((body, checksum)) = line.strip_prefix('$').and_then(|s| s.split_once('*')) else {
            return false;
        };
        if checksum.len() != 2
            || u8::from_str_radix(checksum, 16).ok() != Some(body.bytes().fold(0, |a, b| a ^ b))
        {
            return false;
        }
        let mut fields = body.split(',');
        let Some(kind) = fields.next() else {
            return false;
        };
        if kind.len() != 5 || !kind.bytes().all(|b| b.is_ascii_uppercase()) {
            return false;
        }
        if &kind[2..] == "GGA" {
            let data: heapless::Vec<&str, 20> = fields.clone().take(20).collect();
            let Some(quality) = data
                .get(5)
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|q| *q <= 8)
            else {
                return false;
            };
            let Some(satellites) = data.get(6).and_then(|s| s.parse::<u8>().ok()) else {
                return false;
            };
            let position = if quality != 0 {
                let Some(lat) = data
                    .get(1)
                    .zip(data.get(2))
                    .and_then(|(v, h)| coordinate(v, h, true))
                else {
                    return false;
                };
                let Some(lon) = data
                    .get(3)
                    .zip(data.get(4))
                    .and_then(|(v, h)| coordinate(v, h, false))
                else {
                    return false;
                };
                Some((lat, lon))
            } else {
                None
            };
            self.position = position;
            self.gga_updated = true;
            self.quality = quality;
            self.satellites = satellites;
            self.gga.clear();
            let _ = self.gga.push_str(line);
        }
        if &kind[2..] == "RMC" {
            let data: heapless::Vec<&str, 20> = fields.clone().take(20).collect();
            if data.get(1) == Some(&"A") {
                self.utc_update_ms = data
                    .get(8)
                    .zip(data.first())
                    .and_then(|(date, time)| crate::clock::nmea_unix_ms(date, time));
            }
        }
        self.last_sentence.clear();
        let _ = self.last_sentence.push_str(line);
        true
    }
}

/// Convert NMEA degrees/minutes to signed millionths of a degree without floats.
pub fn coordinate(value: &str, hemisphere: &str, latitude: bool) -> Option<i32> {
    let degrees_len = if latitude { 2 } else { 3 };
    if !value.is_ascii() || value.len() < degrees_len + 2 {
        return None;
    }
    let degrees: u32 = value[..degrees_len].parse().ok()?;
    let minutes = &value[degrees_len..];
    let (whole, fraction) = minutes.split_once('.').unwrap_or((minutes, ""));
    if whole.len() != 2
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let limit = if latitude { 90 } else { 180 };
    if whole >= 60
        || degrees > limit
        || (degrees == limit && (whole != 0 || fraction.bytes().any(|b| b != b'0')))
    {
        return None;
    }
    let mut micro_minutes = whole * 1_000_000;
    for (i, b) in fraction.bytes().take(6).enumerate() {
        micro_minutes += u32::from(b - b'0') * [100000, 10000, 1000, 100, 10, 1][i];
    }
    let result = (degrees * 1_000_000 + (micro_minutes + 30) / 60) as i32;
    match (latitude, hemisphere) {
        (true, "N") | (false, "E") => Some(result),
        (true, "S") | (false, "W") => Some(-result),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sentence(body: &str) -> std::string::String {
        std::format!("${}*{:02X}\r\n", body, body.bytes().fold(0u8, |a, b| a ^ b))
    }
    #[test]
    fn coordinates_and_rmc_clock_validation() {
        assert_eq!(coordinate("4807.038", "N", true), Some(48117300));
        assert_eq!(coordinate("01131.000", "W", false), Some(-11516667));
        assert_eq!(coordinate("9000.000", "S", true), Some(-90000000));
        for (v, h, lat) in [
            ("9060.0", "N", true),
            ("9000.1", "N", true),
            ("18100.0", "E", false),
            ("4807.0", "E", true),
            ("-807.0", "N", true),
        ] {
            assert_eq!(coordinate(v, h, lat), None);
        }
        let mut rx = Receiver::default();
        for b in sentence("GNRMC,123456.789,A,4241.860,N,02319.302,E,0.0,0.0,031026,,,A").bytes() {
            rx.feed(b);
        }
        assert_eq!(
            rx.utc_update_ms,
            crate::clock::nmea_unix_ms("031026", "123456.789")
        );
        rx.feed(b'x');
        assert_eq!(rx.utc_update_ms, None);
        for body in [
            "GNRMC,123456.789,V,,,,,,,031026,,,N",
            "GNRMC,123456,A,,,,,,,310226,,,A",
        ] {
            for b in sentence(body).bytes() {
                rx.feed(b);
            }
            assert_eq!(rx.utc_update_ms, None);
        }
        for b in sentence("GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,").bytes() {
            rx.feed(b);
        }
        assert_eq!(rx.position, Some((48117300, 11516667)));
        for b in sentence("GNGGA,,,,,,0,00,,,,,,,").bytes() {
            rx.feed(b);
        }
        assert_eq!(rx.position, None);
    }

    #[test]
    fn streaming_fix_and_loss() {
        let mut rx = Receiver::default();
        for b in sentence("GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,").bytes() {
            rx.feed(b);
        }
        assert_eq!((rx.valid, rx.quality, rx.satellites), (1, 1, 8));
        for b in b"$GNGGA,broken*00\n" {
            rx.feed(*b);
        }
        assert_eq!((rx.valid, rx.invalid, rx.quality), (1, 1, 1));
        for b in sentence("GPGGA,,,,,,0,00,,,,,,,").bytes() {
            rx.feed(b);
        }
        assert_eq!((rx.valid, rx.quality, rx.satellites), (2, 0, 0));
    }
    #[test]
    fn repeated_gga_and_invalid_fields() {
        let mut rx = Receiver::default();
        let body = "GNGGA,,,,,,0,00,,,,,,,";
        for _ in 0..2 {
            for b in sentence(body).bytes() {
                rx.feed(b);
            }
            assert!(rx.gga_updated);
        }
        for body in ["GNGGA,,,,,,9,08", "GNGGA,,,,,,1,no", "GNGGA,broken"] {
            for b in sentence(body).bytes() {
                rx.feed(b);
            }
            assert!(!rx.gga_updated);
        }
        assert_eq!((rx.valid, rx.invalid, rx.quality), (2, 3, 0));
        for b in sentence("GNRMC,,V,,,,,,,,,,").bytes() {
            rx.feed(b);
        }
        assert!(!rx.gga_updated);
    }

    #[test]
    fn bounded_and_resynchronizes() {
        let mut rx = Receiver::default();
        rx.feed(b'$');
        for _ in 0..200 {
            rx.feed(b'x');
        }
        for b in sentence("GNRMC,,V,,,,,,,,,,").bytes() {
            rx.feed(b);
        }
        assert_eq!((rx.valid, rx.invalid), (1, 1));
        for b in b"$GNGGA,,,,,,9,08*ZZ\n" {
            rx.feed(*b);
        }
        assert_eq!(rx.valid, 1);
    }
}

/// First observed events, in firmware monotonic milliseconds (not physical power-on).
#[derive(Default)]
pub struct Startup {
    pub byte: Option<u64>,
    pub nmea: Option<u64>,
    pub view: Option<u64>,
    pub signal: Option<u64>,
    pub fix: Option<u64>,
    pub utc: Option<u64>,
}
impl Startup {
    pub fn observe(&mut self, now: u64, rx: &Receiver, accepted: bool) {
        self.byte.get_or_insert(now);
        if !accepted {
            return;
        }
        self.nmea.get_or_insert(now);
        if rx.gga_updated && rx.position.is_some() {
            self.fix.get_or_insert(now);
        }
        if rx.utc_update_ms.is_some() {
            self.utc.get_or_insert(now);
        }
        let body = rx.last_sentence.split('*').next().unwrap_or("");
        let fields: heapless::Vec<&str, 24> = body.split(',').collect();
        if fields
            .first()
            .is_some_and(|kind| kind.len() == 6 && kind.ends_with("GSV"))
        {
            if fields
                .get(3)
                .and_then(|s| s.parse::<u8>().ok())
                .is_some_and(|n| n > 0)
            {
                self.view.get_or_insert(now);
            }
            if fields
                .iter()
                .skip(7)
                .step_by(4)
                .any(|s| s.parse::<u8>().is_ok_and(|n| n > 0))
            {
                self.signal.get_or_insert(now);
            }
        }
    }
}
#[cfg(test)]
mod startup_tests {
    use super::*;
    fn feed(rx: &mut Receiver, events: &mut Startup, body: &str, now: u64) {
        let line = std::format!("${}*{:02X}\r\n", body, body.bytes().fold(0u8, |a, b| a ^ b));
        for b in line.bytes() {
            let accepted = rx.feed(b);
            events.observe(now, rx, accepted);
        }
    }
    #[test]
    fn first_events_survive_history_and_loss() {
        let mut rx = Receiver::default();
        let mut events = Startup::default();
        feed(&mut rx, &mut events, "GPGSV,1,1,00", 10);
        assert_eq!(events.byte, Some(10));
        assert_eq!(events.view, None);
        feed(&mut rx, &mut events, "GPGSV,1,1,01,01,10,020,", 20);
        assert_eq!(events.view, Some(20));
        assert_eq!(events.signal, None);
        feed(&mut rx, &mut events, "GPGSV,1,1,01,01,10,020,24", 30);
        assert_eq!(events.signal, Some(30));
        feed(
            &mut rx,
            &mut events,
            "GNGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,",
            40,
        );
        feed(&mut rx, &mut events, "GNRMC,123456,A,,,,,,,031026,,,A", 50);
        for _ in 0..30 {
            feed(&mut rx, &mut events, "GNGGA,,,,,,0,00,,,,,,,", 60);
        }
        assert_eq!(events.fix, Some(40));
        assert_eq!(events.utc, Some(50));
        assert_eq!(events.nmea, Some(10));
        assert_eq!(rx.position, None);
    }
}
