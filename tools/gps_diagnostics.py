#!/usr/bin/env python3
"""Read the board's bounded, checksum-validated NMEA history without changing GNSS settings."""

import argparse
import json
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from pager_tools.hardware import hardware_lock
from pager_tools.protocol_spec import COMMANDS
from pager_tools.usb import PagerUsbClient, find_application, parse_info
from tools.flash_uf2 import get_backend


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--duration", type=float, default=30)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.duration <= 0:
        parser.error("duration must be positive")
    device = find_application(get_backend(), args.serial)
    if device is None:
        parser.exit(1, "selected board not found\n")
    samples = []
    with hardware_lock(args.serial), PagerUsbClient(device) as client:
        info = parse_info(client.call(bytes([COMMANDS["get_info"]])).decode())
        if info.get("board") != "xiao-nrf52840":
            parser.exit(1, "GPS diagnostics supported only on XIAO\n")
        started = time.monotonic()
        while time.monotonic() - started < args.duration:
            state = parse_info(client.call(bytes([COMMANDS["get_gps"]])).decode())
            sentences = []
            for index in range(24):
                raw = client.call(bytes([COMMANDS["get_gps_sentence"], index])).decode()
                if raw:
                    sentences.append(parse_info(raw))
            sample = {"elapsed": time.monotonic() - started, "state": state, "sentences": sentences}
            samples.append(sample)
            print(json.dumps(sample), flush=True)
            time.sleep(2)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"info": info, "samples": samples}, indent=2) + "\n")


if __name__ == "__main__":
    main()
