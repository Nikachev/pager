import pytest
from pathlib import Path
import re

from pager_tools.protocol import (
    FrameKind,
    ProtocolError,
    encode_frame,
    read_frame,
)


def test_frame_round_trip_and_fragmentation():
    encoded = encode_frame(FrameKind.COMMAND, 0xAABBCCDD, b"hello")
    assert read_frame(encoded[:10]) is None
    parsed, remaining = read_frame(encoded + b"tail")
    assert parsed == (FrameKind.COMMAND, 0xAABBCCDD, b"hello")
    assert remaining == b"tail"


def test_frame_resynchronises_after_stale_usb_bytes():
    encoded = encode_frame(FrameKind.RESPONSE, 7, b"state")
    parsed, remaining = read_frame(b"stale partial transfer" + encoded)
    assert parsed == (FrameKind.RESPONSE, 7, b"state")
    assert remaining == b""


def test_frame_rejects_crc_version_and_oversize():
    encoded = bytearray(encode_frame(FrameKind.RESPONSE, 1, b"ok"))
    encoded[-1] ^= 1
    with pytest.raises(ProtocolError, match="CRC"):
        read_frame(bytes(encoded))
    encoded = bytearray(encode_frame(FrameKind.RESPONSE, 1, b"ok"))
    encoded[4] = 4
    with pytest.raises(ProtocolError, match="incompatible"):
        read_frame(bytes(encoded))
    with pytest.raises(ProtocolError, match="512"):
        encode_frame(FrameKind.COMMAND, 1, bytes(513))


def test_ui_artifacts_are_standalone_and_share_design_tokens():
    root = Path(__file__).resolve().parents[1]
    pages = [(root / name).read_text(encoding="utf-8") for name in ("ble_client.html", "webusb_client.html")]
    for page in pages:
        assert not re.search(r"(?:src|href)=[\"']https?://", page)
        assert "--bg:#f6f7f9" in page
        assert "--accent:#2563eb" in page
