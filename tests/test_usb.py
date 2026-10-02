from types import SimpleNamespace

import pytest
import usb.core as usb_core

from pager_tools import usb
from pager_tools.protocol import FrameKind, ProtocolError, encode_frame


def test_serial_case_and_ambiguous_selection(monkeypatch):
    devices = [SimpleNamespace(serial_number="abcdef1234567890")]
    monkeypatch.setattr(usb.usb.core, "find", lambda **_: devices)
    assert usb.find_application(serial="ABCDEF1234567890") is devices[0]
    devices.append(SimpleNamespace(serial_number="other"))
    with pytest.raises(ProtocolError, match="multiple"):
        usb.find_application()
    assert usb.find_application(serial="missing") is None


def test_metadata_fields_exact_and_duplicate_rejection():
    assert usb.parse_info("Pager;version=26.09.1;board=xiao-nrf52840")["version"] == "26.09.1"
    with pytest.raises(ProtocolError, match="duplicate"):
        usb.parse_info("Pager;version=1;version=2")


def test_remaining_deadline_used_after_fragment(monkeypatch):
    client = usb.PagerUsbClient(None)
    client.endpoint_out = SimpleNamespace(write=lambda frame, **_: len(frame))
    frame = encode_frame(FrameKind.RESPONSE, 1, b"ok")
    reads = []
    ticks = [0.0]
    monkeypatch.setattr(usb.time, "monotonic", lambda: ticks[0])

    def read(_size, timeout):
        reads.append(timeout)
        if len(reads) == 1:
            ticks[0] += 0.025
            return frame[:8]
        return frame[8:]

    client.endpoint_in = SimpleNamespace(read=read)
    assert client.call(b"\x01", timeout_ms=100) == b"ok"
    assert reads[1] < reads[0] - 10


def test_usb_timeout_explains_deadline():
    client = usb.PagerUsbClient(None)
    client.endpoint_out = SimpleNamespace(write=lambda frame, **_: len(frame))

    def read(*_, **__):
        raise usb_core.USBTimeoutError("timed out")

    client.endpoint_in = SimpleNamespace(read=read)
    with pytest.raises(ProtocolError, match="timeout"):
        client.call(b"\x01", timeout_ms=10)


def test_cleanup_after_disconnect_and_detach(monkeypatch):
    calls = []
    device = SimpleNamespace(attach_kernel_driver=lambda _: calls.append("attach"))
    client = usb.PagerUsbClient(device)
    client.interface = 3
    client.claimed = client.detached = True

    def release(*_):
        calls.append("release")
        raise usb_core.USBError("disconnected")

    monkeypatch.setattr(usb.usb.util, "release_interface", release)
    monkeypatch.setattr(usb.usb.util, "dispose_resources", lambda _: calls.append("dispose"))
    client.close()
    assert calls == ["release", "attach", "dispose"]


def test_error_response_and_wrong_request_id():
    client = usb.PagerUsbClient(None)
    client.endpoint_out = SimpleNamespace(write=lambda frame, **_: len(frame))
    client.endpoint_in = SimpleNamespace(
        read=lambda *_, **__: encode_frame(FrameKind.ERROR, 1, b"\x03")
    )
    with pytest.raises(ProtocolError, match="command error 3"):
        client.call(b"\x01")
    client.endpoint_in = SimpleNamespace(
        read=lambda *_, **__: encode_frame(FrameKind.RESPONSE, 99, b"ok")
    )
    with pytest.raises(ProtocolError, match="unexpected response id"):
        client.call(b"\x01")
