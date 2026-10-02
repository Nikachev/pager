//! Legacy HID advertising payloads, refreshed for every enable of a slot.
pub struct Payload {
    pub advertising: [u8; 31],
    pub scan_response: [u8; 31],
    pub advertising_len: usize,
    pub scan_response_len: usize,
}

impl Payload {
    pub fn new(pairing: bool, name: &str) -> Option<Self> {
        let name = name.as_bytes();
        if name.len() > 29 {
            return None;
        }
        let mut advertising = [0; 31];
        // Flags: General Discoverable only during pairing, LE-only always.
        advertising[..9].copy_from_slice(&[
            2,
            1,
            if pairing { 6 } else { 4 },
            5,
            3,
            0x12,
            0x18,
            0x0f,
            0x18,
        ]);
        let mut scan_response = [0; 31];
        scan_response[0] = name.len() as u8 + 1;
        scan_response[1] = 9; // Complete Local Name.
        scan_response[2..2 + name.len()].copy_from_slice(name);
        Some(Self {
            advertising,
            scan_response,
            advertising_len: 9,
            scan_response_len: name.len() + 2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_pairing_and_rename_refresh_payloads_without_changing_services() {
        let reconnect = Payload::new(false, "Pager 2").unwrap();
        let pairing = Payload::new(true, "PagerXiao 2").unwrap();
        let reconnect_again = Payload::new(false, "Renamed 2").unwrap();
        assert_eq!(reconnect.advertising[2], 4);
        assert_eq!(pairing.advertising[2], 6);
        assert_eq!(reconnect_again.advertising[2], 4);
        assert_eq!(reconnect.advertising[3..9], pairing.advertising[3..9]);
        assert_eq!(pairing.scan_response[0], 12);
        assert_eq!(
            &pairing.scan_response[2..pairing.scan_response_len],
            b"PagerXiao 2"
        );
        assert_eq!(
            &reconnect_again.scan_response[2..reconnect_again.scan_response_len],
            b"Renamed 2"
        );
    }

    #[test]
    fn legacy_scan_payload_limits_are_utf8_bytes() {
        assert_eq!(
            Payload::new(true, "я".repeat(14).as_str())
                .unwrap()
                .scan_response_len,
            30
        );
        assert!(Payload::new(true, "я".repeat(15).as_str()).is_none());
    }
}
