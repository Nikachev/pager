import json
from types import SimpleNamespace

from tools.gps_recorder import record


def test_armed_before_removal_and_reconnect(tmp_path, monkeypatch):
    now = [0.0]
    devices = iter([object(), None, object()])
    calls = []

    class Client:
        def __init__(self, device):
            calls.append("opened")

        def __enter__(self):
            return self

        def get_info(self):
            return "board=xiao-nrf52840"

        def close(self):
            calls.append("closed")

    def capture(client):
        # Each sample has already been flushed before the next capture.
        if calls.count("sample"):
            assert '"event": "sample"' in args.output.read_text()
        calls.append("sample")
        return dict(state={"fix": "0"}, startup={"first_fix_ms": ""}, sentences=[])

    monkeypatch.setattr("tools.gps_recorder.sample", capture)
    args = SimpleNamespace(
        output=tmp_path / "run.jsonl", cycle=True, serial="A222", wait=10, duration=2, interval=1
    )
    assert (
        record(
            args,
            discover=lambda: next(devices),
            clock=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
            client_type=Client,
        )
        == 0
    )
    events = [json.loads(line)["event"] for line in args.output.read_text().splitlines()]
    assert events == ["armed", "usb_absent", "connected", "sample", "sample", "complete"]
    assert calls == ["opened", "sample", "sample", "closed"]


def test_wait_is_bounded_and_evidence_survives(tmp_path):
    now = [0.0]
    args = SimpleNamespace(
        output=tmp_path / "run.jsonl", cycle=True, serial="A222", wait=1, duration=2, interval=1
    )
    assert (
        record(
            args,
            discover=lambda: None,
            clock=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
        )
        == 2
    )
    rows = [json.loads(line) for line in args.output.read_text().splitlines()]
    assert rows[-1]["event"] == "wait_timeout"
    assert all(row["event"] != "sample" for row in rows)


def test_disconnect_reconnect_retains_original_deadline(tmp_path, monkeypatch):
    from pager_tools.protocol import ProtocolError

    now = [0.0]
    captures = [0]

    class Client:
        def __init__(self, device):
            pass

        def __enter__(self):
            return self

        def get_info(self):
            return "board=xiao-nrf52840"

        def close(self):
            pass

    def capture(client):
        captures[0] += 1
        if captures[0] == 1:
            raise ProtocolError("disconnected")
        return dict(state={"fix": "0"}, startup={}, sentences=[])

    monkeypatch.setattr("tools.gps_recorder.sample", capture)
    args = SimpleNamespace(
        output=tmp_path / "run.jsonl", cycle=False, serial="A222", wait=1, duration=2, interval=1
    )
    assert (
        record(
            args,
            discover=lambda: object(),
            clock=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
            client_type=Client,
        )
        == 0
    )
    rows = [json.loads(line) for line in args.output.read_text().splitlines()]
    assert sum(row["event"] == "connected" for row in rows) == 2
    assert any(row["event"] == "transport_error" for row in rows)
    assert rows[-1]["elapsed_s"] < 3
