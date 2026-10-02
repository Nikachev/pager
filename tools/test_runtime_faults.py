#!/usr/bin/env python3
"""Exercise opt-in one-shot runtime faults on one serial-bound development app.

Requires the exact fault package and a bootloader handing off reset diagnostics.
No HID input, pairing, factory reset, or physical erasure is performed.
"""

import argparse
import json
from pathlib import Path
import sys
import time

import libusb_package
import usb.core

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT))
from pager_tools.hardware import hardware_lock
from pager_tools.protocol import ProtocolError
from pager_tools.usb import PagerUsbClient, find_application


def durable(state):
    return bytes([state[1], state[3]]) + state[6:9] + state[10:]


def device_name(state):
    offset = 10
    for _ in range(7):
        length = state[offset]
        offset += 1
        value = state[offset : offset + length]
        offset += length
    return value


def diagnostic_values(logs):
    line = next((s for s in logs.splitlines() if s.startswith("DIAG:reset=")), "")
    return dict(item.split("=", 1) for item in line.removeprefix("DIAG:").split(";") if "=" in item)


def watchdog_reboot_observed(code, initial_uptime_ms, current_uptime_ms, elapsed_s):
    # A retained watchdog cause from a previous boot is insufficient evidence.
    continuous_uptime = initial_uptime_ms + elapsed_s * 1000
    return bool(code & 2) and current_uptime_ms < continuous_uptime - 5000


def run(serial, package, output, stall, only_stall=False):
    metadata = json.loads(package.with_name("package.json").read_text())
    if not metadata.get("fault_injection"):
        raise ValueError("requires an explicitly marked fault-injection package")
    backend = libusb_package.get_libusb1_backend()
    report = {"serial": serial, "image_sha256": metadata["image_sha256"], "faults": []}

    def save():
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n")

    with hardware_lock(serial):
        device = find_application(backend=backend, serial=serial)
        if device is None:
            raise RuntimeError("selected application not found")
        with PagerUsbClient(device) as client:
            info = client.get_info()
            if (
                f"image_sha256={metadata['image_sha256']}" not in info
                or ";fault_injection=1" not in info
            ):
                raise ValueError("running application is not the exact fault package")
            initial = client.call(bytes([3]))
            original = device_name(initial)
            for action in [] if only_stall else [1, 2, 6, 7]:
                client.call(bytes([0xF0, action]))
                started = time.monotonic()
                try:
                    client.call(bytes([11]) + f"Fault{action}".encode(), timeout_ms=14000)
                except ProtocolError as error:
                    failure = str(error)
                else:
                    raise AssertionError(f"fault {action} reported success")
                elapsed = time.monotonic() - started
                if action in [6, 7] and elapsed < 9.5:
                    raise AssertionError("expected absolute command deadline")
                time.sleep(1.25)
                state = client.call(bytes([3]))
                if action != 7 and durable(state) != durable(initial):
                    raise AssertionError("failed/expired mutation changed durable state")
                client.call(bytes([11]) + b"Recovery", timeout_ms=14000)
                if device_name(client.call(bytes([3]))) != b"Recovery":
                    raise AssertionError("next request did not apply its own mutation")
                client.call(bytes([11]) + original, timeout_ms=14000)
                if durable(client.call(bytes([3]))) != durable(initial):
                    raise AssertionError("original state did not restore")
                report["faults"].append(
                    {
                        "action": action,
                        "error": failure,
                        "elapsed_s": elapsed,
                        "next_request_success": True,
                        "original_state_restored": True,
                    }
                )
                save()
                print(f"fault {action}: passed ({elapsed:.2f}s)", flush=True)
            if stall is None:
                return report
            initial_diag = diagnostic_values(client.call(bytes([10])).decode(errors="replace"))
            if "uptime_ms" not in initial_diag:
                raise RuntimeError("watchdog proof requires live diagnostic uptime")
            initial_uptime = int(initial_diag["uptime_ms"])
            client.call(bytes([0xF0, stall]))
            started = time.monotonic()
            try:
                client.call(bytes([11]) + b"StallPending", timeout_ms=14000)
            except ProtocolError:
                pass
            else:
                raise AssertionError("stalled command reported success")
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            time.sleep(0.5)
            try:
                device = find_application(backend=backend, serial=serial)
                if device is None:
                    continue
                with PagerUsbClient(device) as client:
                    logs = client.call(bytes([10]), timeout_ms=1500).decode(errors="replace")
                    diag = diagnostic_values(logs)
                    if "reset" not in diag or "uptime_ms" not in diag:
                        continue
                    reason = int(diag["reset"], 16)
                    if watchdog_reboot_observed(
                        reason, initial_uptime, int(diag["uptime_ms"]), time.monotonic() - started
                    ):
                        if durable(client.call(bytes([3]))) != durable(initial):
                            raise AssertionError("watchdog reboot changed durable state")
                        report["watchdog"] = {
                            "stall": stall,
                            "reset_code": reason,
                            "initial_uptime_ms": initial_uptime,
                            "after_uptime_ms": int(diag["uptime_ms"]),
                            "elapsed_s": time.monotonic() - started,
                            "durable_state_equal": True,
                        }
                        save()
                        print(f"watchdog stall {stall}: reset verified", flush=True)
                        return report
            except ProtocolError, OSError, usb.core.USBError:
                continue
        report["watchdog_error"] = "watchdog reset was not verified"
        save()
        raise AssertionError(report["watchdog_error"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--stall", type=int, choices=[3, 4])
    parser.add_argument("--only-stall", action="store_true")
    args = parser.parse_args()
    run(args.serial, args.package, args.output, args.stall, args.only_stall)


if __name__ == "__main__":
    main()
