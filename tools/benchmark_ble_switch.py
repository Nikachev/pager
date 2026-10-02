#!/usr/bin/env python3
"""Measure slot 1/2 reconnect latency on a prepared Mac + Android HIL fixture.

Both hosts must be paired and awake. This changes the active slot but never
types, clears bonds, or pairs devices; the original slot is restored at the end.
"""

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import statistics
import sys
import time

import libusb_package

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from pager_tools.hardware import hardware_lock
from pager_tools.usb import PagerUsbClient, find_application


def state(client):
    payload = client.call(bytes([3]))
    if len(payload) < 10 or payload[0] != 5:
        raise RuntimeError("unsupported baseline state schema")
    if not payload[1] or list(payload[6:9]) != [1, 1, 0]:
        raise RuntimeError("requires Bluetooth On, paired slots 1/2 and empty slot 3")
    return payload[:10]


def switch(client, slot, timeout):
    started = time.perf_counter()
    response = client.call(bytes([4, slot]), timeout_ms=12000)
    acknowledged = time.perf_counter()
    if response != b"\x00":
        raise RuntimeError(f"unexpected switch response: {response.hex()}")
    deadline = started + timeout
    while time.perf_counter() < deadline:
        current = state(client)
        if current[3] == slot and current[4] == slot and current[9]:
            return {
                "slot": slot + 1,
                "ack_ms": (acknowledged - started) * 1000,
                "hid_ready_ms": (time.perf_counter() - started) * 1000,
            }
        time.sleep(0.1)
    raise RuntimeError(f"slot {slot + 1} failed to reconnect within {timeout}s")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default=os.getenv("PAGER_USB_SERIAL"))
    parser.add_argument("--cycles", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.cycles < 1 or args.timeout <= 0:
        parser.error("cycles and timeout must be positive")
    device = find_application(backend=libusb_package.get_libusb1_backend(), serial=args.serial)
    if device is None:
        parser.exit(1, "Pager application not found\n")
    report = {
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "serial": device.serial_number,
        "poll_interval_ms": 100,
        "samples": [],
        "status": "running",
    }
    try:
        with hardware_lock(device.serial_number), PagerUsbClient(device) as client:
            report["info"] = client.get_info()
            original = state(client)[3]
            if original not in (0, 1):
                raise RuntimeError("requires original active slot 1 or 2")
            try:
                for _ in range(args.cycles):
                    for slot in (1 - original, original):
                        sample = switch(client, slot, args.timeout)
                        report["samples"].append(sample)
                        print(
                            f"Slot {slot + 1}: ack={sample['ack_ms']:.1f} ms, "
                            f"HID ready={sample['hid_ready_ms']:.1f} ms",
                            flush=True,
                        )
            finally:
                if state(client)[3] != original:
                    switch(client, original, args.timeout)
            values = sorted(sample["hid_ready_ms"] for sample in report["samples"])
            report["summary"] = {
                "count": len(values),
                "p50_ms": statistics.median(values),
                "p95_ms": values[math.ceil(len(values) * 0.95) - 1],
            }
            report["status"] = "passed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
