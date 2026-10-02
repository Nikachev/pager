"""Serial-bound sustained maximum-frame OUT integrity test; never sends HID."""

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


def run(serial, output, batches=128, frames_per_batch=16):
    report = dict(serial=serial, hid_sent=False, status="running", batches=[])

    def save():
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n")

    with hardware_lock(serial):
        device = find_application(backend=libusb_package.get_libusb1_backend(), serial=serial)
        with PagerUsbClient(device) as client:
            report["info"] = client.get_info()
            initial = decode_state(client.call(b"\x03"))
            inbox = queue.Queue()
            stop = threading.Event()

            def reader():
                try:
                    while not stop.is_set():
                        parsed = read_frame(client.buffer)
                        if parsed is not None:
                            answer, client.buffer = parsed
                            inbox.put(answer)
                            continue
                        try:
                            client.buffer += bytes(client.endpoint_in.read(64, timeout=200))
                        except usb.core.USBTimeoutError:
                            continue
                except Exception as error:
                    inbox.put(error)

            worker = threading.Thread(target=reader)
            worker.start()
            rid = 100000
            try:
                for batch in range(batches):
                    expected = {}
                    wire = bytearray()
                    for _ in range(frames_per_batch):
                        # Public synthetic bytes, different across frames/packets.
                        body = b"\xfe" + bytes((rid + i * 37) & 255 for i in range(511))
                        wire.extend(encode_frame(FrameKind.COMMAND, rid, body))
                        expected[rid] = (FrameKind.ERROR, b"\x02")
                        rid += 1
                        wire.extend(encode_frame(FrameKind.COMMAND, rid, b"\x01"))
                        expected[rid] = (FrameKind.RESPONSE, b"PONG")
                        rid += 1
                    started = time.monotonic()
                    assert client.endpoint_out.write(wire, timeout=12000) == len(wire)
                    found = set()
                    deadline = started + 15
                    while len(found) < len(expected):
                        answer = inbox.get(timeout=max(0.001, deadline - time.monotonic()))
                        if isinstance(answer, Exception):
                            raise answer
                        kind, request, body = answer
                        if kind == FrameKind.EVENT:
                            continue
                        assert request in expected and request not in found, (kind, request)
                        assert (kind, body) == expected[request], (kind, request, list(body))
                        found.add(request)
                    report["batches"].append(
                        dict(
                            index=batch,
                            bytes=len(wire),
                            responses=len(found),
                            elapsed_ms=(time.monotonic() - started) * 1000,
                        )
                    )
                    save()
                report["status"] = "passed"
            except Exception as error:
                report.update(
                    status="failed",
                    error=f"{type(error).__name__}: {error}",
                    failed_batch=batch,
                    received_requests=sorted(found),
                    missing_requests=sorted(set(expected) - found),
                )
                raise
            finally:
                stop.set()
                worker.join(timeout=3)
                save()
                assert not worker.is_alive(), "USB reader failed to stop"
            try:
                final = decode_state(client.call(b"\x03"))
                for field in ("enabled", "active", "bonds", "names", "addresses", "base_name"):
                    assert initial[field] == final[field], field
                report["durable_state_preserved"] = True
            except Exception as error:
                report.update(status="failed", error=f"{type(error).__name__}: {error}")
                raise
            finally:
                save()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--batches", type=int, default=128)
    args = parser.parse_args()
    if args.batches < 1:
        parser.error("batches must be positive")
    report = run(args.serial, args.output, args.batches)
    print(f"USB integrity: {report['status']}, {len(report['batches'])} batches")


if __name__ == "__main__":
    main()
