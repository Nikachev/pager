"""Serial-bound, non-HID event/response and overflow qualification.

Repeated slot-name writes use the existing name, retaining durable settings.
One IN reader runs throughout the session, including initial/final snapshots.
"""

import argparse
import json
from pathlib import Path
import queue
import sys
import threading
import time

import libusb_package
import usb.core

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pager_tools.hardware import hardware_lock
from pager_tools.protocol import FrameKind, decode_state, encode_frame, read_frame
from pager_tools.usb import PagerUsbClient, find_application


def run(serial):
    report = dict(serial=serial, status="running", events=[], responses=[], wire_order=[])
    with hardware_lock(serial):
        device = find_application(backend=libusb_package.get_libusb1_backend(), serial=serial)
        if device is None:
            raise RuntimeError("exact application not found")
        with PagerUsbClient(device) as client:
            inbox = queue.Queue()
            stop = threading.Event()

            def reader():
                buffer = b""
                try:
                    while not stop.is_set():
                        parsed = read_frame(buffer)
                        if parsed is not None:
                            frame, buffer = parsed
                            inbox.put(frame)
                            continue
                        try:
                            buffer += bytes(client.endpoint_in.read(64, timeout=200))
                        except usb.core.USBTimeoutError:
                            continue
                except Exception as error:
                    inbox.put(error)

            worker = threading.Thread(target=reader)
            worker.start()
            request = 30000

            def receive(timeout):
                answer = inbox.get(timeout=timeout)
                if isinstance(answer, Exception):
                    raise answer
                kind, rid, body = answer
                report["wire_order"].append(dict(kind=int(kind), id=rid))
                if kind == FrameKind.EVENT:
                    assert rid == 0 and len(body) == 5 and body[0] in (1, 255), answer
                    report["events"].append(
                        dict(type=body[0], revision=int.from_bytes(body[1:], "little"))
                    )
                else:
                    report["responses"].append(dict(id=rid, kind=int(kind)))
                return answer

            def exchange(payloads):
                nonlocal request
                ids = list(range(request, request + len(payloads)))
                request += len(payloads)
                joined = b"".join(
                    encode_frame(FrameKind.COMMAND, rid, payload)
                    for rid, payload in zip(ids, payloads, strict=True)
                )
                assert client.endpoint_out.write(joined, timeout=12000) == len(joined)
                answers = {}
                deadline = time.monotonic() + 30
                while len(answers) != len(ids):
                    kind, rid, body = receive(max(0.001, deadline - time.monotonic()))
                    if kind == FrameKind.EVENT:
                        continue
                    assert rid in ids and rid not in answers, (kind, rid)
                    assert kind == FrameKind.RESPONSE, (kind, rid, list(body))
                    answers[rid] = body
                return [answers[rid] for rid in ids]

            def drain(seconds):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    try:
                        kind, rid, body = receive(max(0.001, deadline - time.monotonic()))
                    except queue.Empty:
                        break
                    assert kind == FrameKind.EVENT, (kind, rid, body)

            try:
                report["info"] = exchange([bytes([2])])[0].decode()
                initial = decode_state(exchange([bytes([3])])[0])
                assert initial["bonds"][0] and initial["names"][0]
                assert initial["enabled"] and initial["active"] == 0, "slot 1 fixture required"
                drain(0.5)
                # Same name writes generate control + persist revisions without
                # changing settings or breaking the existing BLE connection.
                rename = bytes([12, 0]) + initial["names"][0].encode()
                start = len(report["events"])
                for _ in range(3):
                    assert exchange([rename, bytes([1]), bytes([3])])[:2] == [b"\0", b"PONG"]
                    drain(0.2)
                assert len(report["events"]) > start, "no events interleaved with responses"
                report["interleave_passed"] = True
                start = len(report["events"])
                assert exchange([rename] * 32) == [b"\0"] * 32
                drain(1)
                burst = report["events"][start:]
                report["overflow_observed"] = any(event["type"] == 255 for event in burst)
                report["burst_events"] = burst
                revisions = [event["revision"] for event in report["events"] if event["type"] == 1]
                report["revision_gaps"] = [
                    dict(before=before, after=after)
                    for before, after in zip(revisions, revisions[1:], strict=False)
                    if (after - before) & 0xFFFFFFFF != 1
                ]
                report["ble_controls"] = []
                controls = [bytes([6, 0]), bytes([6, 1]), bytes([4, 1]), bytes([4, 0])]
                for control in controls:
                    started = time.monotonic()
                    assert exchange([control, bytes([1])]) == [b"\0", b"PONG"]
                    state = decode_state(exchange([bytes([3])])[0])
                    if control == bytes([6, 0]):
                        assert not state["enabled"] and state["connected"] is None
                    elif control[0] == 4:
                        assert state["active"] == control[1]
                    assert state["bonds"] == initial["bonds"]
                    report["ble_controls"].append(
                        dict(command=list(control), elapsed_ms=(time.monotonic() - started) * 1000)
                    )
                deadline = time.monotonic() + 45
                while True:
                    state = decode_state(exchange([bytes([3]), bytes([1])])[0])
                    if state["connected"] == initial["active"] and state["hid_ready"]:
                        break
                    assert time.monotonic() < deadline, "initial host did not regain HID readiness"
                    drain(0.2)
                report["live_usb_during_ble_controls_passed"] = True
                final = decode_state(exchange([bytes([3])])[0])
                for field in ("enabled", "active", "bonds", "names", "addresses", "base_name"):
                    assert initial[field] == final[field], field
                report["durable_state_preserved"] = True
                report["snapshot_after_burst_passed"] = True
                report["status"] = (
                    "passed" if report["overflow_observed"] else "overflow-not-observed"
                )
                print(f"Event qualification: {report['status']}", flush=True)
            finally:
                stop.set()
                worker.join(timeout=3)
                assert not worker.is_alive(), "USB reader failed to stop"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = run(args.serial)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    if report["status"] != "passed":
        raise SystemExit("overflow was not observed; hardware requirement remains open")


if __name__ == "__main__":
    main()
