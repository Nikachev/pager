"""Host unit tests for scripts/common_dfu.py protocol framing and lock file handling."""

import sys
import os
from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import common_dfu  # noqa: E402


def test_common_dfu_crc32_vector() -> None:
    assert common_dfu.crc32(b"123456789") == 0xCBF43926


def test_common_dfu_frame_roundtrip() -> None:
    payload = b"hello_pager"
    frame = common_dfu.encode_usb_frame(common_dfu.KIND_COMMAND, 100, payload)
    kind, req_id, parsed_payload = common_dfu.parse_usb_frame(frame)
    assert kind == common_dfu.KIND_COMMAND
    assert req_id == 100
    assert parsed_payload == payload


def test_common_dfu_lock_file_path() -> None:
    path = common_dfu.get_lock_file_path()
    assert "pager_hil_test.lock" in path
