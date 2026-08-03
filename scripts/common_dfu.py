"""
Shared DFU and WebUSB/Serial protocol utilities for Pager scripts and tests.
"""

import os
import tempfile
import struct
import zlib

def get_lock_file_path() -> str:
    """Return platform-appropriate lock file path for hardware test access."""
    return os.environ.get("PAGER_LOCK_FILE", os.path.join(tempfile.gettempdir(), "pager_hil_test.lock"))

# Protocol v1 Constants
USB_FRAME_MAGIC = b"PGR1"
USB_FRAME_VERSION = 1
USB_FRAME_HEADER_LEN = 16
USB_MAX_PAYLOAD = 512

KIND_COMMAND = 1
KIND_RESPONSE = 2
KIND_EVENT = 3
KIND_DFU_DATA = 4
KIND_ERROR = 5

OPCODE_PING = 1
OPCODE_GET_INFO = 2
OPCODE_GET_KEYBOARD_STATE = 3
OPCODE_DFU_BEGIN = 9
OPCODE_DFU_COMMIT = 10
OPCODE_DFU_ABORT = 11

ERR_BAD_REQUEST = 1
ERR_UNSUPPORTED_COMMAND = 2
ERR_BUSY = 3
ERR_DFU = 4

def crc32(data: bytes) -> int:
    """Compute IEEE 802.3 CRC32 checksum."""
    return zlib.crc32(data) & 0xFFFFFFFF

def encode_usb_frame(kind: int, request_id: int, payload: bytes) -> bytes:
    """Encode a WebUSB protocol frame into binary format."""
    if len(payload) > USB_MAX_PAYLOAD:
        raise ValueError("Payload size exceeds maximum allowed")
    checksum = crc32(payload)
    return struct.pack(
        "<4sBBIHI",
        USB_FRAME_MAGIC,
        USB_FRAME_VERSION,
        kind,
        request_id,
        len(payload),
        checksum,
    ) + payload

def parse_usb_frame(frame: bytes):
    """Parse and validate a WebUSB protocol frame."""
    if len(frame) < USB_FRAME_HEADER_LEN:
        raise ValueError("Frame too short")
    magic, version, kind, request_id, payload_len, expected_crc = struct.unpack(
        "<4sBBIHI", frame[:16]
    )
    if magic != USB_FRAME_MAGIC or version != USB_FRAME_VERSION:
        raise ValueError("Invalid frame header magic or version")
    payload = frame[USB_FRAME_HEADER_LEN:USB_FRAME_HEADER_LEN + payload_len]
    if len(payload) != payload_len:
        raise ValueError("Payload length mismatch")
    if crc32(payload) != expected_crc:
        raise ValueError("CRC32 mismatch")
    return kind, request_id, payload
