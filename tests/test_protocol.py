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
    pages = [
        (root / name).read_text(encoding="utf-8")
        for name in ("ble_client.html", "webusb_client.html")
    ]
    for page in pages:
        assert not re.search(r"(?:src|href)=[\"']https?://", page)
        assert "--bg:#f6f7f9" in page
        assert "--accent:#2563eb" in page


def test_shared_golden_vectors():
    import json

    vectors = json.loads((Path(__file__).parent / "fixtures/protocol_vectors.json").read_text())
    for vector in vectors:
        payload = bytes.fromhex(vector["payload_hex"])
        frame = bytes.fromhex(vector["frame_hex"])
        assert encode_frame(FrameKind(vector["kind"]), vector["request_id"], payload) == frame
        assert read_frame(frame) == (
            (FrameKind(vector["kind"]), vector["request_id"], payload),
            b"",
        )


def test_protocol_generation_and_validation():
    import importlib.util
    import json

    root = Path(__file__).resolve().parents[1]
    module_spec = importlib.util.spec_from_file_location(
        "generate_protocol", root / "tools/generate_protocol.py"
    )
    generator = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(generator)
    spec = json.loads((root / "protocol.json").read_text())
    generator.validate(spec)
    for path, expected in generator.artifacts(spec).items():
        assert path.read_text() == expected
    spec["commands"]["get_info"] = spec["commands"]["ping"]
    with pytest.raises(ValueError, match="duplicate"):
        generator.validate(spec)


def test_state_truncation_extra_fields_utf8_and_slots():
    from pager_tools.protocol import decode_state

    state = bytes([5, 1, 1, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    assert decode_state(state)["base_name"] == ""
    for size in range(len(state)):
        with pytest.raises(ProtocolError):
            decode_state(state[:size])
    with pytest.raises(ProtocolError, match="trailing"):
        decode_state(state + b"x")
    invalid = bytearray(state)
    invalid[3] = 3
    with pytest.raises(ProtocolError, match="slot"):
        decode_state(bytes(invalid))
    with pytest.raises(ProtocolError, match="UTF-8"):
        decode_state(state[:-1] + b"\x01\xff")


def test_resynchronization_keeps_only_candidate_or_magic_suffix():
    from pager_tools.protocol import resynchronize

    assert resynchronize(b"garbage" * 10000 + b"PG") == b"PG"
    assert resynchronize(b"garbage" * 10000) == b""
    frame = encode_frame(FrameKind.RESPONSE, 7, b"ok")
    assert resynchronize(b"garbage" + frame) == frame
