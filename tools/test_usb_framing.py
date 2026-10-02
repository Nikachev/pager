"""Serial-bound, non-HID raw framing stress for the development device."""

import argparse
import json
from pathlib import Path
import sys
import time
import queue
import threading
import usb.core

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import libusb_package
from pager_tools.hardware import hardware_lock
from pager_tools.protocol import FrameKind, encode_frame, read_frame, decode_state
from pager_tools.usb import PagerUsbClient, find_application


def run(serial):
    results = []
    with hardware_lock(serial):
        device = find_application(backend=libusb_package.get_libusb1_backend(), serial=serial)
        with PagerUsbClient(device) as client:
            info = client.get_info()
            initial = decode_state(client.call(bytes([3])))
            request = 10000

            def exchange(name, chunks, expected):
                nonlocal request
                started = time.monotonic()
                responses = queue.Queue()
                stop = threading.Event()

                def reader():
                    try:
                        while not stop.is_set():
                            parsed = read_frame(client.buffer)
                            if parsed is not None:
                                answer, client.buffer = parsed
                                responses.put(answer)
                                continue
                            try:
                                client.buffer += bytes(client.endpoint_in.read(64, timeout=200))
                            except usb.core.USBTimeoutError:
                                continue
                    except Exception as error:
                        responses.put(error)

                worker = threading.Thread(target=reader)
                worker.start()
                found = {}
                errors = []
                deadline = started + 8
                try:
                    for chunk in chunks:
                        assert client.endpoint_out.write(chunk, timeout=3000) == len(chunk)
                    while set(found) != set(expected):
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            raise AssertionError(f"{name}: missing responses")
                        answer = responses.get(timeout=remaining)
                        if isinstance(answer, Exception):
                            raise answer
                        kind, rid, body = answer
                        if kind == FrameKind.EVENT:
                            continue
                        if rid == 0 and kind == FrameKind.ERROR:
                            errors.append(list(body))
                            continue
                        assert rid in expected and rid not in found, (name, rid)
                        assert (kind, body) == expected[rid], (name, kind, body)
                        found[rid] = True
                finally:
                    stop.set()
                    worker.join(timeout=3)
                    assert not worker.is_alive(), "USB reader failed to stop"
                results.append(
                    dict(name=name, errors=errors, elapsed_ms=(time.monotonic() - started) * 1000)
                )
                print(f"passed {name}", flush=True)
                request += 2

            ping = bytes([1])
            frame = encode_frame(FrameKind.COMMAND, request, ping)
            exchange(
                "zero-length-out-between-fragments",
                [frame[:8], b"", frame[8:]],
                {request: (FrameKind.RESPONSE, b"PONG")},
            )
            for split in range(1, 17):
                frame = encode_frame(FrameKind.COMMAND, request, ping)
                exchange(
                    f"split-{split}",
                    [frame[:split], frame[split:]],
                    {request: (FrameKind.RESPONSE, b"PONG")},
                )
            maximum = encode_frame(FrameKind.COMMAND, request, bytes([0xFE]) * 512)
            following = encode_frame(FrameKind.COMMAND, request + 1, ping)
            exchange(
                "maximum-plus-next",
                [maximum + following],
                {
                    request: (FrameKind.ERROR, bytes([2])),
                    request + 1: (FrameKind.RESPONSE, b"PONG"),
                },
            )
            for name, offset in [("version", 4), ("kind", 5), ("oversize", 11), ("crc", 12)]:
                broken = bytearray(encode_frame(FrameKind.COMMAND, request, ping))
                broken[offset] = 255
                following = encode_frame(FrameKind.COMMAND, request + 1, ping)
                exchange(name, [broken + following], {request + 1: (FrameKind.RESPONSE, b"PONG")})
                assert results[-1]["errors"], f"{name}: no malformed-frame error"
            following = encode_frame(FrameKind.COMMAND, request, ping)
            exchange(
                "garbage-partial-magic",
                [b"noisePG", following[2:]],
                {request: (FrameKind.RESPONSE, b"PONG")},
            )
            frame = encode_frame(FrameKind.COMMAND, request, bytes([3, 0]))
            exchange("strict-command-length", [frame], {request: (FrameKind.ERROR, bytes([1]))})
            final = decode_state(client.call(bytes([3])))
            for field in ("enabled", "active", "bonds", "names", "addresses", "base_name"):
                assert initial[field] == final[field], field
            return dict(
                serial=serial,
                info=info,
                status="passed",
                samples=results,
                durable_state_preserved=True,
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = run(args.serial)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Raw framing: {len(report['samples'])} cases passed on {args.serial}")


if __name__ == "__main__":
    main()
