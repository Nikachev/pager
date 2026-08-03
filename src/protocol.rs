#![allow(dead_code)]

/// IEEE CRC-32 used by both OTA transports to detect corrupted staged data.
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;
pub const PACKAGE_MAGIC: [u8; 8] = *b"PGRFW001";
pub const MANIFEST_LEN: usize = 120;
pub const MANIFEST_PAGE_SIZE: usize = 4096;

/// Maximum raw application binary size (484 KiB).
pub const MAX_IMAGE_SIZE: usize = 484 * 1024;
/// Maximum signed package size including manifest page (488 KiB).
pub const MAX_PACKAGE_SIZE: usize = MANIFEST_PAGE_SIZE + MAX_IMAGE_SIZE;

/// Pager WebUSB framing, independent of the USB transport packet boundaries.
///
/// Frames are `magic | version | kind | request_id | payload_len | crc32 |
/// payload`, all integers little-endian. The CRC covers the payload only.
pub const USB_FRAME_MAGIC: [u8; 4] = *b"PGR1";
pub const USB_FRAME_VERSION: u8 = 1;
pub const USB_FRAME_HEADER_LEN: usize = 16;
pub const USB_MAX_PAYLOAD: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum UsbFrameKind {
    Command = 1,
    Response = 2,
    Event = 3,
    DfuData = 4,
    Error = 5,
}

impl TryFrom<u8> for UsbFrameKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            1 => Ok(Self::Command),
            2 => Ok(Self::Response),
            3 => Ok(Self::Event),
            4 => Ok(Self::DfuData),
            5 => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbFrameHeader {
    pub kind: UsbFrameKind,
    pub request_id: u32,
    pub payload_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbFrameError {
    TooShort,
    BadMagic,
    UnsupportedVersion,
    InvalidKind,
    PayloadTooLarge,
    LengthMismatch,
    BadCrc,
}

/// Parses and validates a complete frame. USB bulk transfers may split or join
/// frames; the WebUSB transport accumulates them before calling this function.
pub fn parse_usb_frame(frame: &[u8]) -> Result<(UsbFrameHeader, &[u8]), UsbFrameError> {
    if frame.len() < USB_FRAME_HEADER_LEN {
        return Err(UsbFrameError::TooShort);
    }
    if frame[..4] != USB_FRAME_MAGIC {
        return Err(UsbFrameError::BadMagic);
    }
    if frame[4] != USB_FRAME_VERSION {
        return Err(UsbFrameError::UnsupportedVersion);
    }
    let kind = UsbFrameKind::try_from(frame[5]).map_err(|_| UsbFrameError::InvalidKind)?;
    let request_id = u32::from_le_bytes(frame[6..10].try_into().unwrap());
    let payload_len = u16::from_le_bytes(frame[10..12].try_into().unwrap()) as usize;
    if payload_len > USB_MAX_PAYLOAD {
        return Err(UsbFrameError::PayloadTooLarge);
    }
    if frame.len() != USB_FRAME_HEADER_LEN + payload_len {
        return Err(UsbFrameError::LengthMismatch);
    }
    let expected_crc = u32::from_le_bytes(frame[12..16].try_into().unwrap());
    let payload = &frame[USB_FRAME_HEADER_LEN..];
    if crc32_finalize(crc32_update(CRC32_INIT, payload)) != expected_crc {
        return Err(UsbFrameError::BadCrc);
    }
    Ok((
        UsbFrameHeader {
            kind,
            request_id,
            payload_len,
        },
        payload,
    ))
}

/// Encodes a frame into a caller-owned fixed buffer. Returns the encoded size.
pub fn encode_usb_frame(
    out: &mut [u8],
    kind: UsbFrameKind,
    request_id: u32,
    payload: &[u8],
) -> Result<usize, UsbFrameError> {
    if payload.len() > USB_MAX_PAYLOAD {
        return Err(UsbFrameError::PayloadTooLarge);
    }
    let len = USB_FRAME_HEADER_LEN + payload.len();
    if out.len() < len {
        return Err(UsbFrameError::LengthMismatch);
    }
    out[..4].copy_from_slice(&USB_FRAME_MAGIC);
    out[4] = USB_FRAME_VERSION;
    out[5] = kind as u8;
    out[6..10].copy_from_slice(&request_id.to_le_bytes());
    out[10..12].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    out[12..16].copy_from_slice(&crc32_finalize(crc32_update(CRC32_INIT, payload)).to_le_bytes());
    out[USB_FRAME_HEADER_LEN..len].copy_from_slice(payload);
    Ok(len)
}

/// Precomputed 256-entry lookup table for IEEE 802.3 CRC-32 (polynomial 0xEDB88320).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            if c & 1 != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        let idx = ((crc ^ (byte as u32)) & 0xFF) as usize;
        crc = CRC32_TABLE[idx] ^ (crc >> 8);
    }
    crc
}

pub const fn crc32_finalize(crc: u32) -> u32 {
    !crc
}

/// Parse exactly eight lowercase/uppercase hexadecimal digits. This is used
/// for the OTA checksum header and deliberately rejects prefixes and suffixes.
pub fn parse_hex_u32(value: &[u8]) -> Option<u32> {
    if value.len() != 8 {
        return None;
    }
    value.iter().try_fold(0u32, |result, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        Some((result << 4) | u32::from(digit))
    })
}

/// Validate transport-visible package structure before rebooting into the
/// bootloader. Signature validation remains the bootloader's responsibility.
pub fn valid_package_envelope(
    manifest_page: &[u8],
    package_len: usize,
    expected_slot: u32,
    max_package_len: usize,
) -> bool {
    if manifest_page.len() != MANIFEST_PAGE_SIZE
        || package_len <= MANIFEST_PAGE_SIZE
        || package_len > max_package_len
    {
        return false;
    }
    let state = u32::from_le_bytes(match manifest_page[0..4].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    });
    let image_len = u32::from_le_bytes(match manifest_page[16..20].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    }) as usize;
    let target_slot = u32::from_le_bytes(match manifest_page[20..24].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    });
    state == u32::MAX
        && manifest_page[4..12] == PACKAGE_MAGIC
        && target_slot == expected_slot
        && image_len > 0
        && image_len <= max_package_len - MANIFEST_PAGE_SIZE
        && package_len == MANIFEST_PAGE_SIZE + image_len
        && manifest_page[MANIFEST_LEN..]
            .iter()
            .all(|byte| *byte == 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_vector_incrementally() {
        let crc = crc32_update(CRC32_INIT, b"1234");
        let crc = crc32_update(crc, b"56789");
        assert_eq!(crc32_finalize(crc), 0xCBF4_3926);
    }

    #[test]
    fn checksum_parser_is_exact() {
        assert_eq!(parse_hex_u32(b"deadBEEF"), Some(0xDEAD_BEEF));
        assert_eq!(parse_hex_u32(b"deadbeef!"), None);
        assert_eq!(parse_hex_u32(b"0xdeadbeef"), None);
    }

    #[test]
    fn usb_frame_round_trip_and_rejects_corruption() {
        let mut frame = [0; USB_FRAME_HEADER_LEN + 3];
        let n = encode_usb_frame(&mut frame, UsbFrameKind::Command, 42, b"get").unwrap();
        let (header, payload) = parse_usb_frame(&frame[..n]).unwrap();
        assert_eq!(header.kind, UsbFrameKind::Command);
        assert_eq!(header.request_id, 42);
        assert_eq!(payload, b"get");
        frame[n - 1] ^= 1;
        assert_eq!(parse_usb_frame(&frame[..n]), Err(UsbFrameError::BadCrc));
    }

    #[test]
    fn manifest_validation_rejects_wrong_slot_and_dirty_padding() {
        let mut page = [0xFF; MANIFEST_PAGE_SIZE];
        page[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        page[4..12].copy_from_slice(&PACKAGE_MAGIC);
        page[16..20].copy_from_slice(&4u32.to_le_bytes());
        page[20..24].copy_from_slice(&1u32.to_le_bytes());
        assert!(valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            1,
            500_000
        ));
        assert!(!valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            0,
            500_000
        ));
        page[MANIFEST_LEN] = 0;
        assert!(!valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            1,
            500_000
        ));
    }
}
