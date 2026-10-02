#!/usr/bin/env python3
"""Validate, transfer and verify a signed Pager application on one selected chip."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import usb.core
import usb.util
import usb.backend.libusb1

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from pager_tools import msc
from pager_tools.hardware import hardware_lock
from pager_tools.package import load_application_package, validate_uf2
from pager_tools.usb import PagerUsbClient, find_application, parse_info, normalize_serial

DEFAULT_VID = 0x239A
DEFAULT_PID = 0x0029

# A separate process makes the deadline effective even when filesystem I/O blocks.
# Exactly one transfer is attempted; a timeout must not restart a partial update.
COPY_WORKER = """
import os, sys
with open(sys.argv[1], 'rb') as src, open(sys.argv[2], 'wb', buffering=0) as dst:
    while chunk := src.read(16 * 1024):
        offset = 0
        while offset < len(chunk):
            written = dst.write(chunk[offset:])
            if not written:
                raise OSError('zero-byte UF2 write')
            offset += written
        os.fsync(dst.fileno())
"""


def copy_to_volume(src, mount_path, timeout=90):
    try:
        result = subprocess.run(
            [sys.executable, "-c", COPY_WORKER, str(src), str(Path(mount_path) / "pager.uf2")],
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        print(f"UF2 copy exceeded {timeout}s; no automatic retry", flush=True)
        return False
    if result.returncode:
        print(f"UF2 copy interrupted: {result.stderr.decode(errors='replace').strip()}", flush=True)
    return result.returncode == 0


def get_backend():
    try:
        import libusb_package

        backend = libusb_package.get_libusb1_backend()
        if backend is not None:
            return backend
    except ImportError, OSError:
        pass
    backend = usb.backend.libusb1.get_backend()
    if backend is None:
        raise RuntimeError("libusb backend unavailable; install tests/requirements.txt")
    return backend


def find_device(backend=None, vid=DEFAULT_VID, pid=DEFAULT_PID, serial=None):
    devices = list(usb.core.find(find_all=True, idVendor=vid, idProduct=pid, backend=backend) or [])
    if serial:
        devices = [
            d for d in devices if normalize_serial(d.serial_number) == normalize_serial(serial)
        ]
    if len(devices) > 1:
        raise RuntimeError("multiple bootloaders found; select --serial")
    if not devices:
        return None, None, None
    return devices[0], vid, pid


def find_volume(serial):
    roots = [Path("/Volumes")]
    user = os.getenv("USER")
    if user:
        roots.extend([Path("/media") / user, Path("/run/media") / user])
    matches = []
    target = normalize_serial(serial)
    # v0.2.0 INFO_UF2 swaps device-ID halves; USB descriptors remain canonical.
    legacy = target[8:] + target[:8]
    for root in roots:
        if not root.is_dir():
            continue
        for path in root.iterdir():
            info = path / "INFO_UF2.TXT"
            if not info.is_file():
                continue
            fields = parse_info(
                info.read_text().replace("\\r\\n", "\n"), separator="\n", delimiter=": "
            )
            if fields.get("Board-ID") == "NRF52840-PAGER" and normalize_serial(
                fields.get("Serial", "")
            ) in (target, legacy):
                matches.append(path)
    if len(matches) > 1:
        raise RuntimeError("multiple boot volumes match the chip")
    return matches[0] if matches else None


def wait_for_application(backend=None, serial=None, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if find_application(backend=backend, serial=serial) is not None:
                return True
        except usb.core.USBError:
            # USB descriptors can be temporarily unavailable during enumeration.
            pass
        time.sleep(0.25)
    return False


def verify_running_application(backend, serial, metadata):
    device = find_application(backend=backend, serial=serial)
    if device is None:
        raise RuntimeError("selected Pager application did not enumerate")
    with PagerUsbClient(device) as client:
        fields = parse_info(client.get_info())
    for field, expected in [
        ("protocol", str(metadata["protocol"])),
        ("version", metadata["version"]),
        ("board", metadata["board"]),
        ("image_sha256", metadata["image_sha256"]),
    ]:
        if fields.get(field) != expected:
            raise RuntimeError(
                f"running application {field}={fields.get(field)!r}, expected {expected!r}"
            )
    print(
        f"Verified {normalize_serial(serial)}: {metadata['board']} {metadata['version']}",
        flush=True,
    )


def copy_and_verify(filename, mount_path, backend, serial, metadata):
    copied = copy_to_volume(filename, mount_path)
    if not wait_for_application(backend, serial):
        raise RuntimeError(
            "UF2 copied, but application did not enumerate"
            if copied
            else "UF2 copy failed and application did not enumerate"
        )
    verify_running_application(backend, serial, metadata)
    if not copied:
        print("Copy interrupted by reset; exact running image confirms success", flush=True)


# Compatibility entry points retain the existing tool API; package/identity
# policy stays here while the MSC transport is independently testable.
validate_csw = msc.validate_csw
send_scsi_write_10 = msc.send_scsi_write_10


def raw_transfer(device, data):
    return msc.raw_transfer(device, data, send=send_scsi_write_10)


def _flash_uf2(filename, vid=DEFAULT_VID, pid=DEFAULT_PID, *, serial=None, board=None):
    # Fail before USB discovery/reboot on malformed or mismatched packages.
    data, metadata = load_application_package(filename, REPO_ROOT, board)
    if metadata.get("updater"):
        raise ValueError("bootloader updater requires its dedicated install/verify/restore tool")
    backend = get_backend()
    serial = serial or os.getenv("PAGER_USB_SERIAL")
    application = find_application(backend=backend, serial=serial)
    if application is not None:
        serial = application.serial_number
        with PagerUsbClient(application) as client:
            current = parse_info(client.get_info())
            if current.get("board") and current["board"] != metadata["board"]:
                raise RuntimeError("running device board does not match package")
            if not current.get("board") and board != metadata["board"]:
                raise RuntimeError("legacy app requires explicit --board before upgrading")
            client.call(bytes([9]))
        print(f"Requested bootloader on {normalize_serial(serial)}", flush=True)
    else:
        bootloader, _, _ = find_device(backend, vid, pid, serial)
        if bootloader is None:
            raise RuntimeError("selected Pager application or bootloader not found")
        serial = bootloader.serial_number
        if board != metadata["board"]:
            raise RuntimeError("bootloader requires explicit --board")
    serial = normalize_serial(serial)
    deadline = time.monotonic() + 15
    device = None
    while time.monotonic() < deadline:
        device, _, _ = find_device(backend, vid, pid, serial)
        if device is not None:
            break
        time.sleep(0.25)
    if device is None:
        raise RuntimeError("selected chip did not enter bootloader")
    # Give the OS a bounded opportunity to mount; do not compete with its MSC driver.
    mount_deadline = time.monotonic() + 4
    volume = None
    while time.monotonic() < mount_deadline:
        volume = find_volume(serial)
        if volume is not None:
            break
        time.sleep(0.25)
    started = time.monotonic()
    if volume is not None:
        print(f"Transferring {len(data)} bytes to {volume}", flush=True)
        with tempfile.NamedTemporaryFile(suffix=".uf2") as snapshot:
            snapshot.write(data)
            snapshot.flush()
            copy_and_verify(snapshot.name, volume, backend, serial, metadata)
    else:
        raw_transfer(device, data)
        if not wait_for_application(backend, serial):
            raise RuntimeError("UF2 sent, but selected chip did not boot")
        verify_running_application(backend, serial, metadata)
    print(f"Update verified in {time.monotonic() - started:.2f}s", flush=True)


def flash_uf2(filename, vid=DEFAULT_VID, pid=DEFAULT_PID, *, serial=None, board=None):
    _, metadata = load_application_package(filename, REPO_ROOT, board)
    if metadata.get("updater"):
        raise ValueError("bootloader updater requires its dedicated install/verify/restore tool")
    backend = get_backend()
    serial = serial or os.getenv("PAGER_USB_SERIAL")
    application = find_application(backend=backend, serial=serial)
    device = application or find_device(backend, vid, pid, serial)[0]
    if device is None:
        raise RuntimeError("selected Pager not found")
    with hardware_lock(device.serial_number):
        _flash_uf2(filename, vid, pid, serial=device.serial_number, board=board)


def flash_uf2_bytes(data, vid=DEFAULT_VID, pid=DEFAULT_PID):
    # Kept for malformed-package host tests. Signed updates require their metadata.
    layout = json.loads((REPO_ROOT / "layout.json").read_text())
    paths = [
        REPO_ROOT / "bootloader/firmware_signing_public.hex",
        REPO_ROOT / "keys/dev_signing_public.hex",
    ]
    keys = [bytes.fromhex(p.read_text().strip()) for p in paths if p.exists()]
    validate_uf2(data, layout, keys)
    raise ValueError("a signed application update requires a file with package.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--file", "-f", required=True, help="UF2 inside a board/mode package directory"
    )
    parser.add_argument("--vid", type=lambda value: int(value, 16), default=DEFAULT_VID)
    parser.add_argument("--pid", type=lambda value: int(value, 16), default=DEFAULT_PID)
    parser.add_argument("--serial", default=os.getenv("PAGER_USB_SERIAL"))
    parser.add_argument(
        "--board", default=os.getenv("PAGER_BOARD"), choices=["nice-nano-v2", "xiao-nrf52840"]
    )
    args = parser.parse_args()
    try:
        flash_uf2(args.file, args.vid, args.pid, serial=args.serial, board=args.board)
    except (OSError, ValueError, RuntimeError, usb.core.USBError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
