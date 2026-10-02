#!/usr/bin/env python3
"""Install a chip-bound signed bootloader updater, verify boot flash, restore app."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
import time

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from pager_tools.hardware import hardware_lock
from pager_tools.package import load_application_package
from pager_tools.usb import PagerUsbClient, find_application, normalize_serial, parse_info
from tools.flash_uf2 import (
    copy_to_volume,
    find_device,
    find_volume,
    flash_uf2,
    get_backend,
)


def preflight(updater_file, restore_file, board, serial):
    data, metadata = load_application_package(updater_file, REPO_ROOT, board)
    restore_data, restore = load_application_package(restore_file, REPO_ROOT, board)
    if restore.get("updater"):
        raise ValueError("restore package must be an ordinary Pager application")
    details = metadata.get("updater")
    if not isinstance(details, dict) or details.get("schema") != 1:
        raise ValueError("package is not a supported bootloader updater")
    if details.get("serial") != normalize_serial(serial):
        raise ValueError("updater is bound to another chip")
    boot = details["bootloader"]
    if boot.get("board") != board or boot.get("kind") != "bootloader":
        raise ValueError("embedded bootloader board/kind mismatch")
    if restore["key_fingerprint"] not in boot.get("trusted_key_fingerprints", []):
        raise ValueError("new bootloader does not trust the restore application key")
    embedded = Path(updater_file).resolve().with_name("embedded-bootloader.bin").read_bytes()
    digest = hashlib.sha256(embedded).hexdigest()
    if (
        len(embedded) != details["embedded_len"]
        or digest != details["embedded_sha256"]
        or digest != boot["image_sha256"]
        or not 8 <= len(embedded) <= 0xC000
        or len(embedded) % 4
    ):
        raise ValueError("embedded bootloader length/hash mismatch")
    stack, reset = struct.unpack_from("<II", embedded)
    if not (0x20000000 < stack <= 0x20040000 and stack % 8 == 0):
        raise ValueError("embedded bootloader stack vector is invalid")
    if reset & 1 == 0 or (reset & ~1) >= len(embedded):
        raise ValueError("embedded bootloader reset vector is invalid")
    partition_hash = hashlib.sha256(embedded + b"\xff" * (0xC000 - len(embedded))).hexdigest()
    if partition_hash != boot["partition_sha256"]:
        raise ValueError("embedded bootloader partition hash mismatch")
    blocks = sorted(
        (data[i : i + 512] for i in range(0, len(data), 512)),
        key=lambda block: struct.unpack_from("<I", block, 20)[0],
    )
    image = b"".join(block[32 : 32 + struct.unpack_from("<I", block, 16)[0]] for block in blocks)
    descriptor = b"PGRBLUP1" + struct.pack("<Q", int(serial, 16)) + bytes.fromhex(digest)
    descriptor += struct.pack("<II", 1 if board == "nice-nano-v2" else 2, len(embedded))
    if image.count(descriptor) != 1 or image.count(embedded) != 1:
        raise ValueError("signed updater does not contain the exact target descriptor/image")
    return data, metadata, restore_data, restore


def wait_boot(backend, serial, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        device = find_device(backend, serial=serial)[0]
        volume = find_volume(serial)
        if device is not None and volume is not None:
            return device, volume
        time.sleep(0.25)
    raise RuntimeError("selected chip did not expose PAGER_BOOT; recovery may be required")


def verify_boot_info(text, metadata, serial):
    fields = parse_info(text.replace("\\r\\n", "\n"), "\n", ":")
    boot = metadata["updater"]["bootloader"]
    expected = {
        "Serial": normalize_serial(serial),
        "Board": metadata["board"],
        "Kind": metadata["mode"],
        "Version": boot["version"],
        "Partition-SHA256": boot["partition_sha256"],
    }
    for name, value in expected.items():
        actual = fields.get(name)
        if name == "Partition-SHA256" and actual is not None:
            actual = actual.lower()
        if actual != value:
            raise RuntimeError(f"bootloader verification failed: {name}")
    if "bootloader-updater-v1" not in fields.get("Capabilities", "").split(","):
        raise RuntimeError("bootloader updater capability missing")
    return fields


def validate_storage_transition(before, after, expect_reset=False):
    if len(after) < 10 or after[0] != 5:
        raise RuntimeError("invalid restored state")
    if expect_reset:
        expected = bytes([5, 0, 0, 255, 255, 0, 0, 0, 0, 0]) + bytes(6) + b"\x05Pager"
        if after != expected:
            raise RuntimeError("restored app did not produce the explicitly expected fresh state")
    elif before is not None:

        def persistent(state):
            return bytes([state[1], state[3]]) + state[6:9] + state[10:]

        if len(before) < 10 or before[0] != 5 or persistent(before) != persistent(after):
            raise RuntimeError("bootloader restored app but persistent state changed")


def install(
    updater_file, restore_file, board, serial, report_file, verify_only=False, expect_reset=False
):
    serial = normalize_serial(serial)
    data, metadata, restore_data, restore_metadata = preflight(
        updater_file, restore_file, board, serial
    )
    backend = get_backend()
    with hardware_lock(serial):
        app = find_application(backend=backend, serial=serial)
        before = None
        before_state = None
        if app is not None and verify_only:
            raise RuntimeError("verify/restore-only requires the selected chip in bootloader")
        if app is not None:
            with PagerUsbClient(app) as client:
                info = parse_info(client.get_info())
                if info.get("board") != board:
                    raise RuntimeError("install requires a board-identifying Pager application")
                before_state = client.call(bytes([3]))
                before = list(before_state[:10])
                client.call(bytes([9]))
        device, volume = wait_boot(backend, serial)
        # This is a single transfer. No automatic retry after replacement begins.
        started = time.monotonic()
        copied = None
        if not verify_only:
            with tempfile.NamedTemporaryFile(suffix=".uf2") as snapshot:
                snapshot.write(data)
                snapshot.flush()
                copied = copy_to_volume(snapshot.name, volume)
        # The old volume may still be present while the one-shot app is running.
        # Require the exact new identity/hash; a stale mount never proves success.
        deadline = time.monotonic() + 35
        verified = None
        while time.monotonic() < deadline:
            selected, mounted = find_device(backend, serial=serial)[0], find_volume(serial)
            if selected is not None and mounted is not None:
                try:
                    verified = verify_boot_info(
                        (mounted / "INFO_UF2.TXT").read_text(), metadata, serial
                    )
                    break
                except OSError, RuntimeError:
                    pass
            time.sleep(0.25)
        if verified is None:
            raise RuntimeError("exact new bootloader not verified; no retry or restore attempted")
        print(f"Bootloader verified: {serial} {verified['Version']}", flush=True)
        # Preserve the already-validated restore bytes and metadata in a snapshot.
        with tempfile.TemporaryDirectory() as folder:
            restore_path = Path(folder) / "pager.uf2"
            restore_path.write_bytes(restore_data)
            restore_path.with_name("package.json").write_text(json.dumps(restore_metadata))
            flash_uf2(str(restore_path), board=board, serial=serial)
        with PagerUsbClient(find_application(backend=backend, serial=serial)) as client:
            after_state = client.call(bytes([3]))
            after = list(after_state[:10])
        validate_storage_transition(before_state, after_state, expect_reset)
        report = {
            "serial": serial,
            "bootloader": verified,
            "before": before,
            "after": after,
            "copy_completed": copied,
            "expected_storage_reset": expect_reset,
            "elapsed_s": time.monotonic() - started,
            "restore": restore_metadata,
        }
        report_file.parent.mkdir(parents=True, exist_ok=True)
        report_file.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Bootloader and restored application verified; saved {report_file}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--updater", type=Path, required=True)
    parser.add_argument("--restore", type=Path, required=True)
    parser.add_argument("--board", choices=["nice-nano-v2", "xiao-nrf52840"], required=True)
    parser.add_argument(
        "--serial",
        default=os.getenv("PAGER_USB_SERIAL"),
        required=not os.getenv("PAGER_USB_SERIAL"),
    )
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument(
        "--verify-restore-only",
        action="store_true",
        help="verify already installed bootloader and restore app without sending updater",
    )
    parser.add_argument(
        "--expect-storage-reset",
        action="store_true",
        help="require exact fresh-device state after an announced schema reset",
    )
    args = parser.parse_args()
    try:
        install(
            args.updater,
            args.restore,
            args.board,
            args.serial,
            args.report,
            args.verify_restore_only,
            args.expect_storage_reset,
        )
    except (ValueError, RuntimeError, OSError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
