"""Host unit tests for Python DFU mock frame processing and retry logic."""

import sys
from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import common_dfu  # noqa: E402


class MockUsbTransport:
    """Mock WebUSB endpoint transport for offline protocol testing."""

    def __init__(self):
        self.sent_frames = []

    def send_frame(self, kind: int, request_id: int, payload: bytes) -> bytes:
        frame = common_dfu.encode_usb_frame(kind, request_id, payload)
        self.sent_frames.append(frame)
        # Mock device pong or ok response
        response_payload = b"PONG" if payload == b"PING" else b"\x00"
        return common_dfu.encode_usb_frame(common_dfu.KIND_RESPONSE, request_id, response_payload)


def test_mock_usb_ping() -> None:
    transport = MockUsbTransport()
    resp_bytes = transport.send_frame(common_dfu.KIND_COMMAND, 1, b"PING")
    kind, req_id, payload = common_dfu.parse_usb_frame(resp_bytes)
    assert kind == common_dfu.KIND_RESPONSE
    assert req_id == 1
    assert payload == b"PONG"


def test_mock_dfu_chunk_validation() -> None:
    chunk = b"\x00" * 500
    frame = common_dfu.encode_usb_frame(common_dfu.KIND_DFU_DATA, 42, chunk)
    kind, req_id, payload = common_dfu.parse_usb_frame(frame)
    assert kind == common_dfu.KIND_DFU_DATA
    assert req_id == 42
    assert len(payload) == 500
