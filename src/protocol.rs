/// IEEE CRC-32 used by the USB frame transport.
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;

/// Pager WebUSB framing, independent of the USB transport packet boundaries.
///
/// Frames are `magic | version | kind | request_id | payload_len | crc32 |
/// payload`, all integers little-endian. The CRC covers the payload only.
pub const USB_FRAME_MAGIC: [u8; 4] = *b"PGR1";
pub mod spec {
    include!(concat!(env!("OUT_DIR"), "/protocol_spec.rs"));
}

pub const USB_FRAME_VERSION: u8 = spec::FRAME_VERSION;
pub const USB_FRAME_HEADER_LEN: usize = 16;
pub const USB_MAX_PAYLOAD: usize = spec::MAX_PAYLOAD;

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
    let request_id = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]);
    let payload_len = u16::from_le_bytes([frame[10], frame[11]]) as usize;
    if payload_len > USB_MAX_PAYLOAD {
        return Err(UsbFrameError::PayloadTooLarge);
    }
    if frame.len() != USB_FRAME_HEADER_LEN + payload_len {
        return Err(UsbFrameError::LengthMismatch);
    }
    let expected_crc = u32::from_le_bytes([frame[12], frame[13], frame[14], frame[15]]);
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
    fn framing_round_trips_payload_boundaries_and_request_ids() {
        let mut payload = [0u8; USB_MAX_PAYLOAD];
        let mut frame = [0u8; USB_FRAME_HEADER_LEN + USB_MAX_PAYLOAD];
        for len in [0, 1, 2, 15, 16, 63, 64, 255, 256, USB_MAX_PAYLOAD] {
            for (index, byte) in payload[..len].iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(37).wrapping_add(len as u8);
            }
            for request_id in [0, 1, 0x7fff_ffff, u32::MAX] {
                let encoded = encode_usb_frame(
                    &mut frame,
                    UsbFrameKind::Command,
                    request_id,
                    &payload[..len],
                )
                .unwrap();
                let (header, decoded) = parse_usb_frame(&frame[..encoded]).unwrap();
                assert_eq!(header.request_id, request_id);
                assert_eq!(header.payload_len, len);
                assert_eq!(decoded, &payload[..len]);
            }
        }
    }
}
