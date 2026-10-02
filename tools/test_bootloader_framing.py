"""Serial-bound application UF2 refusal/recovery HIL, through the mounted MSC drive.

The trusted ordinary restore package is verified before touching hardware.
Malformed trials target only its application envelope. Never sends an updater.
"""

import argparse
import json
from pathlib import Path
import struct
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pager_tools.hardware import hardware_lock
from pager_tools.package import load_application_package
from pager_tools.protocol import decode_state
from pager_tools.usb import PagerUsbClient, find_application, parse_info
from tools.flash_uf2 import (
    copy_to_volume,
    find_device,
    find_volume,
    flash_uf2,
    get_backend,
    verify_running_application,
    wait_for_application,
)
from tools.update_bootloader import wait_boot

ROOT = Path(__file__).resolve().parents[1]


def durable(state):
    return {
        key: state[key] for key in ("enabled", "active", "bonds", "names", "addresses", "base_name")
    }


def run(serial, board, restore, output):
    data, metadata = load_application_package(restore, ROOT, board)
    assert not metadata.get("updater"), "ordinary app required"
    backend = get_backend()
    report = dict(serial=serial, board=board, status="running", samples=[])

    def save():
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n")

    def snapshot():
        with PagerUsbClient(find_application(backend=backend, serial=serial)) as client:
            info = client.get_info()
            assert parse_info(info)["board"] == board
            return info, durable(decode_state(client.call(bytes([3]))))

    def reboot():
        with PagerUsbClient(find_application(backend=backend, serial=serial)) as client:
            client.call(bytes([9]))
        _, volume = wait_boot(backend, serial)
        return volume

    def copy(payload, volume):
        with tempfile.NamedTemporaryFile(suffix=".uf2") as trial:
            trial.write(payload)
            trial.flush()
            return copy_to_volume(trial.name, volume)

    with hardware_lock(serial):
        report["initial_info"], initial = snapshot()
        report["restore_version"] = metadata["version"]
        blocks = [data[i : i + 512] for i in range(0, len(data), 512)]
        blocks.sort(key=lambda block: struct.unpack_from("<I", block, 20)[0])
        bad_signature = bytearray(blocks[0])
        bad_signature[32 + 48] ^= 1
        bad_flags = bytearray(blocks[0])
        struct.pack_into("<I", bad_flags, 8, 0)
        bad_bounds = bytearray(blocks[0])
        struct.pack_into("<I", bad_bounds, 12, metadata["layout"]["storage_start"])
        corrupt = bytearray(b"".join(blocks))
        corrupt[512 + 32 + 24] ^= 1
        trials = [
            ("invalid-signature", bytes(bad_signature)),
            ("invalid-flags", bytes(bad_flags)),
            ("storage-address-refused", bytes(bad_bounds)),
            ("omitted-last-block", b"".join(blocks[:-1])),
            ("image-digest-refused", bytes(corrupt)),
        ]
        try:
            for name, payload in trials:
                print(f"starting {name}", flush=True)
                volume = reboot()
                sample = dict(name=name, status="running")
                report["samples"].append(sample)
                save()
                sample["copy_completed"] = copy(payload, volume)
                time.sleep(0.5)
                assert find_application(backend=backend, serial=serial) is None, (
                    "refused/incomplete image unexpectedly booted"
                )
                assert find_device(backend, serial=serial)[0] is not None
                assert find_volume(serial) is not None
                sample["stayed_in_bootloader"] = True
                flash_uf2(str(restore), board=board, serial=serial)
                _, after = snapshot()
                assert json.dumps(initial, sort_keys=True) == json.dumps(after, sort_keys=True)
                sample["ordinary_app_recovery_verified"] = True
                sample["durable_state_preserved"] = True
                sample["status"] = "passed"
                save()
                print(f"passed {name}", flush=True)
            # Deliberate reversed order and exact duplicate, with block zero last.
            volume = reboot()
            payload = blocks[-1] + blocks[-1] + b"".join(reversed(blocks[:-1]))
            sample = dict(name="reordered-and-duplicate", copy_completed=copy(payload, volume))
            report["samples"].append(sample)
            assert wait_for_application(backend, serial), "reordered valid package did not boot"
            verify_running_application(backend, serial, metadata)
            _, after = snapshot()
            assert json.dumps(initial, sort_keys=True) == json.dumps(after, sort_keys=True)
            sample.update(status="passed", durable_state_preserved=True)
            report["status"] = "passed"
            print("passed reordered-and-duplicate", flush=True)
        except Exception as error:
            report.update(status="failed", error_type=type(error).__name__, error=str(error))
            raise
        finally:
            save()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--board", choices=("nice-nano-v2", "xiao-nrf52840"), required=True)
    parser.add_argument("--restore", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    run(args.serial, args.board, args.restore, args.output)


if __name__ == "__main__":
    main()
