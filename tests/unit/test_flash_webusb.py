"""Host-only unit tests for WebUSB framing and protocol v1 encoding/decoding."""

import sys
from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import flash_webusb  # noqa: E402


def test_webusb_crc32_standard() -> None:
    data = b"123456789"
    assert flash_webusb.crc32(data) == 0xCBF43926


def test_webusb_frame_encode_and_parse_roundtrip() -> None:
    payload = b"test_payload"
    request_id = 42
    kind = flash_webusb.KIND_COMMAND

    frame = flash_webusb.encode_usb_frame(kind, request_id, payload)
    assert len(frame) == flash_webusb.USB_FRAME_HEADER_LEN + len(payload)
    assert frame[:4] == b"PGR1"

    parsed_kind, parsed_id, parsed_payload = flash_webusb.parse_usb_frame(frame)
    assert parsed_kind == kind
    assert parsed_id == request_id
    assert parsed_payload == payload


def test_webusb_frame_rejects_bad_crc() -> None:
    payload = b"test_payload"
    frame = bytearray(flash_webusb.encode_usb_frame(flash_webusb.KIND_COMMAND, 1, payload))
    frame[-1] ^= 0xFF
    with pytest.raises(ValueError, match="CRC32 mismatch"):
        flash_webusb.parse_usb_frame(bytes(frame))


def test_webusb_frame_rejects_oversized_payload() -> None:
    oversized = b"A" * 513
    with pytest.raises(ValueError, match="Payload size exceeds maximum"):
        flash_webusb.encode_usb_frame(flash_webusb.KIND_COMMAND, 1, oversized)
