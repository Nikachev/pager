#!/usr/bin/env python3
"""Record read-only Pager USB latency samples without typing or changing slots."""

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


def measure(client, payload, expected, samples):
    timings = []
    for _ in range(samples):
        started = time.perf_counter_ns()
        reply = client.call(payload)
        elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
        if expected is not None and reply != expected:
            raise RuntimeError("unexpected benchmark response")
        timings.append(elapsed_ms)
    ordered = sorted(timings)
    return {
        "samples_ms": timings,
        "count": len(timings),
        "p50_ms": statistics.median(timings),
        "p95_ms": ordered[math.ceil(len(ordered) * 0.95) - 1],
        "mean_ms": statistics.mean(timings),
        "min_ms": ordered[0],
        "max_ms": ordered[-1],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default=os.getenv("PAGER_USB_SERIAL"))
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be positive")
    backend = libusb_package.get_libusb1_backend()
    device = find_application(backend=backend, serial=args.serial)
    if device is None:
        parser.exit(1, "Pager application not found; benchmark did not change hardware\n")
    report = {
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "serial": device.serial_number,
        "python": sys.version.split()[0],
        "percentile_method": "p50 median; p95 nearest rank",
        "commands": {},
    }
    with hardware_lock(device.serial_number), PagerUsbClient(device) as client:
        report["info"] = client.get_info()
        state = client.call(bytes([3]))
        if len(state) < 10 or state[0] != 5:
            raise RuntimeError("unsupported baseline state schema")
        # Only the public state header is retained; no bonds, aliases or keys.
        report["state_header"] = list(state[:10])
        for name, payload, expected in [
            ("ping", bytes([1]), b"PONG"),
            ("get_info", bytes([2]), report["info"].encode()),
            ("get_state", bytes([3]), None),
        ]:
            # Warm the same connection before recording steady-state latency.
            for _ in range(5):
                client.call(payload)
            result = measure(client, payload, expected, args.samples)
            report["commands"][name] = result
            print(f"{name}: p50={result['p50_ms']:.3f} ms, p95={result['p95_ms']:.3f} ms")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
