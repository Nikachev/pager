/// IEEE CRC-32 used by the USB frame transport.
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;

/// Pager WebUSB framing, independent of the USB transport packet boundaries.
///
/// Frames are `magic | version | kind | request_id | payload_len | crc32 |
/// payload`, all integers little-endian. The CRC covers the payload only.
pub const USB_FRAME_MAGIC: [u8; 4] = spec::FRAME_MAGIC;
#[allow(dead_code)]
pub mod spec {
    include!(concat!(env!("OUT_DIR"), "/protocol_spec.rs"));
}

pub const USB_FRAME_VERSION: u8 = spec::FRAME_VERSION;
pub const USB_FRAME_HEADER_LEN: usize = spec::HEADER_SIZE;
pub const USB_MAX_PAYLOAD: usize = spec::MAX_PAYLOAD;

/// Public image identity uses exactly two lowercase hexadecimal digits per byte.
pub fn append_hex<const N: usize>(
    output: &mut heapless::String<N>,
    bytes: &[u8],
) -> Result<(), heapless::CapacityError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char)?;
        output.push(HEX[(byte & 15) as usize] as char)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum UsbFrameKind {
    Command = spec::KIND_COMMAND,
    Response = spec::KIND_RESPONSE,
    Event = spec::KIND_EVENT,
    DfuData = spec::KIND_DFU_DATA,
    Error = spec::KIND_ERROR,
}

impl TryFrom<u8> for UsbFrameKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            spec::KIND_COMMAND => Ok(Self::Command),
            spec::KIND_RESPONSE => Ok(Self::Response),
            spec::KIND_EVENT => Ok(Self::Event),
            spec::KIND_DFU_DATA => Ok(Self::DfuData),
            spec::KIND_ERROR => Ok(Self::Error),
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

#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Ping,
    GetInfo,
    GetState,
    ActivateSlot(u8),
    CancelPairing,
    SetBluetooth(bool),
    ClearSlot(u8),
    TypeText(&'a str),
    Reboot,
    GetLogs,
    SetDeviceName(&'a str),
    SetSlotName(u8, &'a str),
    FactoryReset,
}
#[derive(Debug, PartialEq, Eq)]
pub enum CommandError {
    BadRequest,
    Unsupported,
}

/// Validate the entire payload before any command can have side effects.
pub fn decode_command(bytes: &[u8]) -> Result<Command<'_>, CommandError> {
    use CommandError::*;
    fn text(bytes: &[u8], limit: usize, nonempty: bool) -> Result<&str, CommandError> {
        let value = core::str::from_utf8(bytes).map_err(|_| BadRequest)?;
        if bytes.len() > limit || (nonempty && value.trim().is_empty()) {
            return Err(BadRequest);
        }
        Ok(value)
    }
    Ok(match bytes {
        [spec::PING] => Command::Ping,
        [spec::GET_INFO] => Command::GetInfo,
        [spec::GET_STATE] => Command::GetState,
        [spec::ACTIVATE_SLOT, slot @ 0..=2] => Command::ActivateSlot(*slot),
        [spec::CANCEL_PAIRING] => Command::CancelPairing,
        [spec::SET_BLUETOOTH_ENABLED, enabled @ 0..=1] => Command::SetBluetooth(*enabled != 0),
        [spec::CLEAR_SLOT, slot @ 0..=2] => Command::ClearSlot(*slot),
        [spec::TYPE_TEXT, rest @ ..] => {
            Command::TypeText(text(rest, spec::LIMIT_TYPE_TEXT, false)?)
        }
        [spec::REBOOT_TO_BOOTLOADER] => Command::Reboot,
        [spec::GET_LOGS] => Command::GetLogs,
        [spec::SET_DEVICE_NAME, rest @ ..] => {
            Command::SetDeviceName(text(rest, spec::LIMIT_DEVICE_NAME, true)?)
        }
        [spec::SET_SLOT_NAME, slot @ 0..=2, rest @ ..] => {
            Command::SetSlotName(*slot, text(rest, spec::LIMIT_SLOT_NAME, true)?)
        }
        [spec::FACTORY_RESET] => Command::FactoryReset,
        [id, ..] if spec::PING <= *id && *id <= spec::FACTORY_RESET => return Err(BadRequest),
        [] => return Err(BadRequest),
        _ => return Err(Unsupported),
    })
}

pub fn encode_state(state: &crate::ble::KeyboardState) -> heapless::Vec<u8, 256> {
    let mut payload = heapless::Vec::new();
    // Capacities of names and peer addresses bound the encoded state below 256.
    let _ = payload.extend_from_slice(&[
        spec::STATE_SCHEMA,
        state.bluetooth_enabled as u8,
        state.link_state as u8,
        state.active_profile.map(|s| s as u8).unwrap_or(255),
        state.connected_profile.map(|s| s as u8).unwrap_or(255),
        state.pairing_mode as u8,
        state.bonds[0].is_some() as u8,
        state.bonds[1].is_some() as u8,
        state.bonds[2].is_some() as u8,
        state.hid_ready as u8,
    ]);
    for i in 0..3 {
        let name = if state.bonds[i].is_some() {
            state.slot_names[i].as_bytes()
        } else {
            &[]
        };
        let _ = payload.push(name.len() as u8);
        let _ = payload.extend_from_slice(name);
    }
    for bond in &state.bonds {
        let address = bond
            .as_ref()
            .map(|b| crate::ble::format_default_peer_name(b.identity.addr.addr.raw()));
        let bytes = address.as_ref().map(|s| s.as_bytes()).unwrap_or(&[]);
        let _ = payload.push(bytes.len() as u8);
        let _ = payload.extend_from_slice(bytes);
    }
    let _ = payload.push(state.device_name.len() as u8);
    let _ = payload.extend_from_slice(state.device_name.as_bytes());
    payload
}

/// Fixed-memory byte stream parser. A USB packet can contain the end of one
/// maximum frame and the start of another; callers drain between bounded slices.
pub struct FrameStream {
    bytes: [u8; USB_FRAME_HEADER_LEN + USB_MAX_PAYLOAD],
    len: usize,
}

pub struct OwnedFrame {
    pub header: UsbFrameHeader,
    bytes: heapless::Vec<u8, USB_MAX_PAYLOAD>,
}
impl OwnedFrame {
    pub const fn new() -> Self {
        Self {
            header: UsbFrameHeader {
                kind: UsbFrameKind::Command,
                request_id: 0,
                payload_len: 0,
            },
            bytes: heapless::Vec::new(),
        }
    }
    pub fn payload(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}
impl Default for OwnedFrame {
    fn default() -> Self {
        Self::new()
    }
}
impl Default for FrameStream {
    fn default() -> Self {
        Self::new()
    }
}
impl FrameStream {
    pub const fn new() -> Self {
        Self {
            bytes: [0; USB_FRAME_HEADER_LEN + USB_MAX_PAYLOAD],
            len: 0,
        }
    }
    pub fn reset(&mut self) {
        self.len = 0;
    }
    /// Append a bounded part of a transfer, then drain before appending more.
    /// This handles a maximum frame followed by another header in one packet
    /// without parsing (and returning a large owned value) after every byte.
    pub fn push_slice(&mut self, input: &[u8]) -> Result<usize, UsbFrameError> {
        let count = input.len().min(self.bytes.len() - self.len);
        if count == 0 && !input.is_empty() {
            return Err(UsbFrameError::PayloadTooLarge);
        }
        self.bytes[self.len..self.len + count].copy_from_slice(&input[..count]);
        self.len += count;
        Ok(count)
    }
    fn discard(&mut self, count: usize) {
        self.bytes.copy_within(count..self.len, 0);
        self.len -= count;
    }
    /// Resynchronization scans at most one fixed buffer, retaining partial magic.
    /// Header errors are rejected before waiting for their declared body.
    #[cfg(test)]
    pub fn next_frame(&mut self) -> Option<Result<OwnedFrame, UsbFrameError>> {
        let mut output = OwnedFrame::new();
        self.next_frame_into(&mut output)
            .map(|result| result.map(|()| output))
    }

    /// Write into caller-owned storage so tiny frames do not move a whole
    /// maximum-capacity Vec through return values. The output owns its bytes
    /// independently of the stream; incomplete/error results leave it intact.
    pub fn next_frame_into(
        &mut self,
        output: &mut OwnedFrame,
    ) -> Option<Result<(), UsbFrameError>> {
        while self.len > 0 {
            let prefix = self.len.min(USB_FRAME_MAGIC.len());
            if self.bytes[..prefix] != USB_FRAME_MAGIC[..prefix] {
                self.discard(1);
                continue;
            }
            if self.len < USB_FRAME_HEADER_LEN {
                return None;
            }
            let length = u16::from_le_bytes([self.bytes[10], self.bytes[11]]) as usize;
            let error = if self.bytes[4] != USB_FRAME_VERSION {
                Some(UsbFrameError::UnsupportedVersion)
            } else if UsbFrameKind::try_from(self.bytes[5]).is_err() {
                Some(UsbFrameError::InvalidKind)
            } else if length > USB_MAX_PAYLOAD {
                Some(UsbFrameError::PayloadTooLarge)
            } else {
                None
            };
            if let Some(error) = error {
                self.discard(1);
                return Some(Err(error));
            }
            let total = USB_FRAME_HEADER_LEN + length;
            if self.len < total {
                return None;
            }
            match parse_usb_frame(&self.bytes[..total]) {
                Ok((header, payload)) => {
                    // Own only initialized payload bytes. Tiny control frames do
                    // not need to clear an entire maximum-sized frame buffer.
                    output.bytes.clear();
                    // parse_usb_frame already checked the capacity.
                    output.bytes.extend_from_slice(payload).unwrap();
                    output.header = header;
                    self.discard(total);
                    return Some(Ok(()));
                }
                Err(error) => {
                    self.discard(1);
                    return Some(Err(error));
                }
            }
        }
        None
    }
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
    fn command_lengths_slots_utf8_and_limits_are_strict() {
        assert_eq!(
            decode_command(&[spec::REBOOT_TO_BOOTLOADER, 0]),
            Err(CommandError::BadRequest)
        );
        assert_eq!(
            decode_command(&[spec::ACTIVATE_SLOT, 3]),
            Err(CommandError::BadRequest)
        );
        assert_eq!(
            decode_command(&[spec::SET_BLUETOOTH_ENABLED, 2]),
            Err(CommandError::BadRequest)
        );
        assert_eq!(
            decode_command(&[spec::SET_DEVICE_NAME, 255]),
            Err(CommandError::BadRequest)
        );
        assert_eq!(
            decode_command(&[spec::SET_DEVICE_NAME, b' ']),
            Err(CommandError::BadRequest)
        );
        assert_eq!(decode_command(&[0xfe]), Err(CommandError::Unsupported));
        assert_eq!(
            decode_command(&[spec::TYPE_TEXT]),
            Ok(Command::TypeText(""))
        );
        let mut bytes = std::vec![spec::TYPE_TEXT];
        bytes.extend([b'x'; 256]);
        assert!(decode_command(&bytes).is_ok());
        bytes.push(b'x');
        assert_eq!(decode_command(&bytes), Err(CommandError::BadRequest));
    }

    #[test]
    fn shared_golden_frames_match_wire_contract() {
        fn hex(value: &str) -> std::vec::Vec<u8> {
            value
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect()
        }
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/protocol_vectors.json")).unwrap();
        for vector in vectors.as_array().unwrap() {
            let payload = hex(vector["payload_hex"].as_str().unwrap());
            let expected = hex(vector["frame_hex"].as_str().unwrap());
            let mut encoded = std::vec![0; expected.len()];
            let kind = UsbFrameKind::try_from(vector["kind"].as_u64().unwrap() as u8).unwrap();
            let id = vector["request_id"].as_u64().unwrap() as u32;
            encode_usb_frame(&mut encoded, kind, id, &payload).unwrap();
            assert_eq!(encoded, expected);
            let (header, decoded) = parse_usb_frame(&expected).unwrap();
            assert_eq!(header.request_id, id);
            assert_eq!(decoded, payload);
        }
    }

    fn framed(payload: &[u8], id: u32) -> std::vec::Vec<u8> {
        let mut bytes = std::vec![0; USB_FRAME_HEADER_LEN + payload.len()];
        encode_usb_frame(&mut bytes, UsbFrameKind::Command, id, payload).unwrap();
        bytes
    }
    fn feed(parser: &mut FrameStream, bytes: &[u8], output: &mut std::vec::Vec<u32>) -> usize {
        let mut errors = 0;
        for &byte in bytes {
            parser.push_slice(&[byte]).unwrap();
            while let Some(result) = parser.next_frame() {
                match result {
                    Ok(frame) => output.push(frame.header.request_id),
                    Err(_) => errors += 1,
                }
            }
        }
        errors
    }
    #[test]
    fn stream_all_split_positions_and_maximum_joined_frames() {
        let mut bytes = framed(&[0x55; USB_MAX_PAYLOAD], u32::MAX);
        bytes.extend(framed(b"next", 0));
        for split in 0..=bytes.len() {
            let mut parser = FrameStream::new();
            let mut output = std::vec::Vec::new();
            assert_eq!(feed(&mut parser, &bytes[..split], &mut output), 0);
            assert_eq!(feed(&mut parser, &bytes[split..], &mut output), 0);
            assert_eq!(output, [u32::MAX, 0]);
        }
    }
    #[test]
    fn transfer_chunks_preserve_frames_payloads_and_resynchronize() {
        let lengths = [0, 1, 63, 64, 255, 256, USB_MAX_PAYLOAD, 1];
        let payloads: std::vec::Vec<std::vec::Vec<u8>> = lengths
            .iter()
            .map(|length| (0..*length).map(|i| (i as u8).wrapping_mul(37)).collect())
            .collect();
        let mut wire = std::vec::Vec::new();
        let mut broken = framed(b"bad-crc", 99);
        broken[12] ^= 1;
        wire.extend(b"garbage");
        wire.extend(broken);
        for (id, payload) in payloads.iter().enumerate() {
            wire.extend(framed(payload, id as u32));
        }
        for chunk_size in [1, 2, 16, 64, 127, 528, 900] {
            let mut parser = FrameStream::new();
            let mut frame = OwnedFrame::new();
            let mut received = std::vec::Vec::new();
            let mut errors = 0;
            for chunk in wire.chunks(chunk_size) {
                let mut offset = 0;
                while offset < chunk.len() {
                    offset += parser.push_slice(&chunk[offset..]).unwrap();
                    while let Some(result) = parser.next_frame_into(&mut frame) {
                        match result {
                            Ok(()) => {
                                received.push((frame.header.request_id, frame.payload().to_vec()))
                            }
                            Err(_) => errors += 1,
                        }
                    }
                }
            }
            assert_eq!(errors, 1);
            assert_eq!(
                received,
                payloads
                    .iter()
                    .enumerate()
                    .map(|(id, payload)| (id as u32, payload.clone()))
                    .collect::<std::vec::Vec<_>>()
            );
            parser.reset();
            assert_eq!(parser.push_slice(&[]), Ok(0));
        }
    }
    #[test]
    fn output_buffer_survives_partial_error_and_reset_then_reuses_shorter_payload() {
        let mut parser = FrameStream::new();
        let mut output = OwnedFrame::new();
        let maximum = framed(&[0x5a; USB_MAX_PAYLOAD], 1);
        parser.push_slice(&maximum).unwrap();
        assert_eq!(parser.next_frame_into(&mut output), Some(Ok(())));
        parser.reset();
        assert_eq!(output.payload(), &[0x5a; USB_MAX_PAYLOAD]);
        let mut broken = framed(b"broken", 2);
        broken[12] ^= 1;
        parser.push_slice(&broken[..9]).unwrap();
        assert_eq!(parser.next_frame_into(&mut output), None);
        parser.push_slice(&broken[9..]).unwrap();
        assert_eq!(
            parser.next_frame_into(&mut output),
            Some(Err(UsbFrameError::BadCrc))
        );
        assert_eq!(output.header.request_id, 1);
        assert_eq!(output.payload(), &[0x5a; USB_MAX_PAYLOAD]);
        parser.reset();
        parser.push_slice(&framed(b"x", 3)).unwrap();
        assert_eq!(parser.next_frame_into(&mut output), Some(Ok(())));
        assert_eq!(output.header.request_id, 3);
        assert_eq!(output.payload(), b"x");
    }
    #[test]
    fn owned_payload_survives_following_frames_and_parser_reset() {
        for length in [0, 1, 16, 64, 255, 256, USB_MAX_PAYLOAD] {
            let payload: std::vec::Vec<u8> = (0..length)
                .map(|index| (index as u8).wrapping_mul(37))
                .collect();
            let mut parser = FrameStream::new();
            let mut owned = None;
            for byte in framed(&payload, 42) {
                parser.push_slice(&[byte]).unwrap();
                if let Some(result) = parser.next_frame() {
                    owned = Some(result.unwrap());
                }
            }
            let owned = owned.unwrap();
            let mut following = std::vec::Vec::new();
            assert_eq!(
                feed(&mut parser, &framed(b"different", 43), &mut following),
                0
            );
            parser.reset();
            assert_eq!(following, [43]);
            assert_eq!(owned.header.payload_len, length);
            assert_eq!(owned.payload(), payload);
        }
    }
    #[test]
    fn stream_rejects_bad_headers_without_waiting_and_recovers_crc_garbage() {
        for offset in [4, 5, 11, 12] {
            let mut broken = framed(b"broken", 1);
            broken[offset] = 0xff;
            let mut bytes = std::vec::Vec::from(&b"garbagePGRPGR1"[..]);
            bytes.extend(broken);
            bytes.extend(framed(b"valid", 2));
            let mut parser = FrameStream::new();
            let mut output = std::vec::Vec::new();
            assert!(feed(&mut parser, &bytes, &mut output) > 0);
            assert_eq!(output, [2]);
        }
    }
    #[test]
    fn stream_retains_partial_magic_and_reset_discards_partial_frame() {
        let bytes = framed(b"valid", 7);
        let mut parser = FrameStream::new();
        let mut output = std::vec::Vec::new();
        feed(&mut parser, b"noisePG", &mut output);
        feed(&mut parser, &bytes[2..], &mut output);
        assert_eq!(output, [7]);
        feed(&mut parser, &bytes[..9], &mut output);
        parser.reset();
        feed(&mut parser, &bytes, &mut output);
        assert_eq!(output, [7, 7]);
    }

    #[test]
    fn public_digest_hex_matches_fixed_width_formatting_for_every_byte() {
        let bytes: std::vec::Vec<u8> = (0..=255).collect();
        let expected: std::string::String = bytes.iter().map(|b| std::format!("{b:02x}")).collect();
        let mut output = heapless::String::<512>::new();
        append_hex(&mut output, &bytes).unwrap();
        assert_eq!(output.as_str(), expected);
        let mut too_small = heapless::String::<1>::new();
        assert!(append_hex(&mut too_small, &[0xff]).is_err());
    }
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
