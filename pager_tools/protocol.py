"""Pager protocol v5 framing independent of PyUSB transport packet sizes."""

from enum import IntEnum
import struct
import zlib

from . import protocol_spec as spec

MAGIC = spec.FRAME_MAGIC.encode("ascii")
VERSION = spec.FRAME_VERSION
HEADER_SIZE = spec.HEADER_SIZE
MAX_PAYLOAD = spec.MAX_PAYLOAD
FrameKind = IntEnum("FrameKind", {name.upper(): value for name, value in spec.FRAME_KINDS.items()})
ErrorCode = IntEnum("ErrorCode", {name.upper(): value for name, value in spec.ERRORS.items()})


class ProtocolError(RuntimeError):
    pass


def encode_frame(kind: FrameKind, request_id: int, payload: bytes) -> bytes:
    if len(payload) > MAX_PAYLOAD:
        raise ProtocolError("payload exceeds 512 bytes")
    return (
        struct.pack(
            "<4sBBIHI",
            MAGIC,
            VERSION,
            int(kind),
            request_id,
            len(payload),
            zlib.crc32(payload) & 0xFFFFFFFF,
        )
        + payload
    )


def resynchronize(buffer: bytes) -> bytes:
    """Retain a candidate frame or only a partial magic suffix."""
    start = buffer.find(MAGIC)
    if start >= 0:
        return buffer[start:]
    for length in range(min(len(MAGIC) - 1, len(buffer)), 0, -1):
        if buffer[-length:] == MAGIC[:length]:
            return buffer[-length:]
    return b""


def read_frame(buffer: bytes):
    buffer = resynchronize(buffer)
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


def decode_state(payload: bytes):
    """Decode one exact state snapshot; never accept truncation or extra fields."""
    if len(payload) < 10 or payload[0] != spec.STATE_SCHEMA:
        raise ProtocolError("incompatible state schema")
    if any(payload[i] > 1 for i in (1, 5, 6, 7, 8, 9)) or payload[2] > 6:
        raise ProtocolError("invalid state fields")
    if any(payload[i] != 255 and payload[i] >= spec.LIMITS["slots"] for i in (3, 4)):
        raise ProtocolError("invalid state slot")
    offset = 10

    def string(limit):
        nonlocal offset
        if offset >= len(payload):
            raise ProtocolError("truncated state string")
        size = payload[offset]
        offset += 1
        if size > limit or offset + size > len(payload):
            raise ProtocolError("invalid state string length")
        try:
            value = payload[offset : offset + size].decode("utf-8")
        except UnicodeDecodeError as error:
            raise ProtocolError("invalid state UTF-8") from error
        offset += size
        return value

    names = [string(spec.LIMITS["slot_name"]) for _ in range(spec.LIMITS["slots"])]
    addresses = [string(32) for _ in range(spec.LIMITS["slots"])]
    base = string(spec.LIMITS["device_name"])
    if offset != len(payload):
        raise ProtocolError("trailing state bytes")
    return dict(
        enabled=bool(payload[1]),
        link=payload[2],
        active=None if payload[3] == 255 else payload[3],
        connected=None if payload[4] == 255 else payload[4],
        pairing=bool(payload[5]),
        bonds=tuple(bool(v) for v in payload[6:9]),
        hid_ready=bool(payload[9]),
        names=names,
        addresses=addresses,
        base_name=base,
    )
