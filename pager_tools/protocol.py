"""Pager protocol v5 framing independent of PyUSB transport packet sizes."""

from enum import IntEnum
import struct
import zlib

MAGIC = b"PGR1"
VERSION = 5
HEADER_SIZE = 16
MAX_PAYLOAD = 512


class FrameKind(IntEnum):
    COMMAND = 1
    RESPONSE = 2
    EVENT = 3
    ERROR = 5


class ErrorCode(IntEnum):
    BAD_REQUEST = 1
    UNSUPPORTED_COMMAND = 2
    BUSY = 3
    DFU_FAILURE = 4
    HID_NOT_READY = 5
    UNSUPPORTED_CHARACTER = 6
    CONNECTION_LOST = 7
    QUEUE_FULL = 8


class ProtocolError(RuntimeError):
    pass


def encode_frame(kind: FrameKind, request_id: int, payload: bytes) -> bytes:
    if len(payload) > MAX_PAYLOAD:
        raise ProtocolError("payload exceeds 512 bytes")
    return struct.pack(
        "<4sBBIHI",
        MAGIC,
        VERSION,
        int(kind),
        request_id,
        len(payload),
        zlib.crc32(payload) & 0xFFFFFFFF,
    ) + payload


def read_frame(buffer: bytes):
    # A browser or a previous native client can leave a partial transfer in the
    # host-side USB queue.  Bulk transfer boundaries are not frame boundaries,
    # so retain a possible partial magic suffix and resynchronise on the next
    # complete frame instead of rejecting an otherwise compatible device.
    if not buffer.startswith(MAGIC):
        start = buffer.find(MAGIC, 1)
        if start >= 0:
            buffer = buffer[start:]
        else:
            keep = min(len(MAGIC) - 1, len(buffer))
            suffix = buffer[-keep:] if keep else b""
            for length in range(len(suffix), 0, -1):
                if suffix[-length:] == MAGIC[:length]:
                    return None
            return None
    if len(buffer) < HEADER_SIZE:
        return None
    magic, version, kind, request_id, length, checksum = struct.unpack(
        "<4sBBIHI", buffer[:HEADER_SIZE]
    )
    if version != VERSION:
        raise ProtocolError("incompatible Pager protocol")
    if length > MAX_PAYLOAD:
        raise ProtocolError("invalid Pager payload length")
    total = HEADER_SIZE + length
    if len(buffer) < total:
        return None
    payload = buffer[HEADER_SIZE:total]
    if zlib.crc32(payload) & 0xFFFFFFFF != checksum:
        raise ProtocolError("Pager frame CRC mismatch")
    try:
        frame_kind = FrameKind(kind)
    except ValueError as error:
        raise ProtocolError(f"invalid Pager frame kind {kind}") from error
    return (frame_kind, request_id, payload), buffer[total:]
