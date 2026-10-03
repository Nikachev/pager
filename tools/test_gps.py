#!/usr/bin/env python3
"""Check L76K stream over WebUSB; optionally require a fresh satellite fix."""

import argparse
import json
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from pager_tools.hardware import hardware_lock
from pager_tools.protocol_spec import COMMANDS
from pager_tools.usb import PagerUsbClient, find_application, parse_info
from tools.flash_uf2 import get_backend


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default=os.getenv("PAGER_USB_SERIAL"))
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument(
        "--wait-device", type=float, default=0, help="Wait for selected board to power up"
    )
    parser.add_argument("--require-fix", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    wait_deadline = time.monotonic() + args.wait_device
    while True:
        device = find_application(get_backend(), args.serial)
        if device is not None or time.monotonic() >= wait_deadline:
            break
        time.sleep(0.25)
    if device is None:
        parser.exit(1, "Pager application not found\n")
    samples = []
    with hardware_lock(device.serial_number), PagerUsbClient(device) as client:
        info = parse_info(client.call(bytes([COMMANDS["get_info"]])).decode())
        if info.get("board") != "xiao-nrf52840":
            parser.exit(1, "L76K supported only on XIAO\n")
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            state = parse_info(client.call(bytes([COMMANDS["get_gps"]])).decode())
            samples.append(state)
            print(json.dumps(state), flush=True)
            first = samples[0]
            passed = (
                len(samples) >= 3
                and state.get("connected") == "1"
                and int(state.get("valid", 0)) - int(first.get("valid", 0)) >= 3
                and int(state.get("gga_age_ms", 999999)) < 5000
                and state.get("invalid") == first.get("invalid")
                and state.get("uart_errors") == first.get("uart_errors")
                and (not args.require_fix or state.get("fix") == "1")
            )
            if passed:
                break
            time.sleep(1)
        else:
            passed = False
    report = {
        "serial": device.serial_number,
        "info": info,
        "passed": passed,
        "require_fix": args.require_fix,
        "samples": samples,
    }
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print("PASS" if passed else "FAIL: no clean live GGA stream/fix within deadline")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
