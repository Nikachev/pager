#!/usr/bin/env python3
"""Install the Pager bootloader through the stock XIAO nRF52840 UF2 drive."""

import argparse
import os
import re
import shutil
from pathlib import Path
import struct
import sys
import time

import usb.core

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools.flash_uf2 import DEFAULT_PID, DEFAULT_VID, get_backend

FACTORY_APPLICATION_START = 0x00027000
FACTORY_BOOTLOADER_START = 0x000F4000
NRF52840_FAMILY_ID = 0xADA52840
UF2_MAGIC_START0 = 0x0A324655
UF2_MAGIC_START1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
FACTORY_VOLUME_NAMES = ("XIAO-BOOT", "XIAO-SENSE")
STOCK_BOARDS = {
    "Seeed_XIAO_nRF52840": 0x0044,
    "Seeed_XIAO_nRF52840_Sense": 0x0045,
    "nRF52840-SeeedXiao-v1": 0x0044,
    "nRF52840-SeeedXiaoSense-v1": 0x0045,
}
# Only the reviewed, unprotected application handoff is supported. The on-device
# ACL check remains authoritative even for vendor builds carrying this version.
SUPPORTED_BOOTLOADERS = {"0.6.1", "0.6.2"}
SUPPORTED_SOFTDEVICES = {"S140 7.3.0", "S140 version 7.3.0"}


def validate_installer_uf2(data: bytes) -> int:
    if not data or len(data) % 512:
        raise ValueError("installer is not a whole number of UF2 blocks")
    blocks = len(data) // 512
    if FACTORY_APPLICATION_START + blocks * 256 > FACTORY_BOOTLOADER_START:
        raise ValueError("installer overlaps the factory bootloader")
    for number, block in enumerate(
        data[offset : offset + 512] for offset in range(0, len(data), 512)
    ):
        magic0, magic1 = struct.unpack_from("<II", block, 0)
        (flags,) = struct.unpack_from("<I", block, 8)
        address, payload_len, block_number, block_count, family = struct.unpack_from(
            "<IIIII", block, 12
        )
        (magic_end,) = struct.unpack_from("<I", block, 508)
        if (magic0, magic1, magic_end) != (
            UF2_MAGIC_START0,
            UF2_MAGIC_START1,
            UF2_MAGIC_END,
        ):
            raise ValueError(f"invalid UF2 magic in block {number}")
        if family != NRF52840_FAMILY_ID:
            raise ValueError(f"block {number} is not for nRF52840")
        if flags != 0x2000:
            raise ValueError(f"invalid UF2 flags in block {number}")
        if block_number != number or block_count != blocks:
            raise ValueError(f"invalid UF2 numbering in block {number}")
        if payload_len != 256:
            raise ValueError(f"invalid payload length in block {number}")
        if address != FACTORY_APPLICATION_START + number * 256:
            raise ValueError(f"block {number} has an unexpected target address")
    return blocks


def stock_volume_candidates():
    roots = [Path("/Volumes")]
    user = os.getenv("USER")
    if user:
        roots.extend((Path("/media") / user, Path("/run/media") / user))
    for root in roots:
        for name in FACTORY_VOLUME_NAMES:
            yield root / name


def find_stock_volume():
    volumes = [path for path in stock_volume_candidates() if path.is_dir()]
    if len(volumes) > 1:
        raise RuntimeError("multiple stock XIAO drives found; connect only the target board")
    return volumes[0] if volumes else None


def validate_stock_info(info: str) -> int:
    # Seeed's factory 0.6.1 image contains literal escaped line endings in the
    # static header, followed by real line endings in the SoftDevice field.
    info = info.replace("\\r\\n", "\n")
    version = re.search(r"^UF2 Bootloader v?([^\s]+)", info)
    fields = dict(line.split(": ", 1) for line in info.splitlines() if ": " in line)
    if fields.get("Board-ID") not in STOCK_BOARDS:
        raise RuntimeError("INFO_UF2.TXT does not identify a supported XIAO nRF52840")
    if version is None or version[1] not in SUPPORTED_BOOTLOADERS:
        raise RuntimeError(
            "unsupported stock bootloader; only reviewed versions 0.6.1/0.6.2 are allowed"
        )
    if fields.get("SoftDevice") not in SUPPORTED_SOFTDEVICES:
        raise RuntimeError("unsupported SoftDevice; installer requires S140 7.3.0 at 0x27000")
    return STOCK_BOARDS[fields["Board-ID"]]


def stock_usb_serial(pid: int, backend=None) -> str:
    devices = [
        device
        for device in usb.core.find(find_all=True, idVendor=0x2886, backend=backend)
        if device.idProduct in STOCK_BOARDS.values()
    ]
    if len(devices) != 1 or devices[0].idProduct != pid:
        raise RuntimeError("connect exactly one stock XIAO matching the selected UF2 drive")
    serial = devices[0].serial_number or ""
    if re.fullmatch(r"[0-9A-Fa-f]{16}", serial) is None:
        raise RuntimeError("cannot read the target XIAO chip serial; installation refused")
    return serial.upper()


def pager_bootloader_ready(serial: str, backend=None) -> bool:
    for device in usb.core.find(
        find_all=True, idVendor=DEFAULT_VID, idProduct=DEFAULT_PID, backend=backend
    ):
        try:
            if (
                device.serial_number or ""
            ).upper() == serial and device.product == "Pager Boot Drive":
                return True
        except usb.core.USBError, ValueError:
            # Enumeration may expose the device before descriptors are readable.
            continue
    return False


def copy_installer(source: Path, volume: Path) -> bool:
    destination = volume / source.name
    time.sleep(0.5)
    try:
        with source.open("rb") as src, destination.open("wb") as dst:
            shutil.copyfileobj(src, dst, length=16 * 1024)
            dst.flush()
            os.fsync(dst.fileno())
        return True
    except OSError as error:
        # UF2 drives often unmount while the host still considers the copy active.
        print(f"Installer copy/flush interrupted: {error}")
        return False


def install(source: Path, volume: Path | None = None, timeout: float = 20.0):
    data = source.read_bytes()
    blocks = validate_installer_uf2(data)
    volume = volume or find_stock_volume()
    if volume is None or not volume.is_dir():
        raise RuntimeError(
            "stock XIAO UF2 drive not found; connect the board and double-press RESET"
        )
    pid = validate_stock_info((volume / "INFO_UF2.TXT").read_text())
    backend = get_backend()
    serial = stock_usb_serial(pid, backend)
    if pager_bootloader_ready(serial, backend):
        raise RuntimeError("target already appears as Pager; installation refused")

    print(f"Found supported stock XIAO bootloader at {volume} (chip {serial})")
    print(f"Copying {source} ({blocks} blocks). Do not disconnect power until PAGER_BOOT appears.")
    copy_ok = copy_installer(source, volume)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if pager_bootloader_ready(serial, backend):
            if not copy_ok:
                print("The stock drive reset during copy; USB enumeration confirms success.")
            print("Pager bootloader installed; PAGER_BOOT is ready for `make flash`.")
            return
        time.sleep(0.25)
    if copy_ok:
        raise RuntimeError("installer copied, but the Pager bootloader did not enumerate")
    raise RuntimeError("installer copy failed and the Pager bootloader did not enumerate")


def main():
    parser = argparse.ArgumentParser(
        description="Destructively replace the stock XIAO nRF52840 MBR/SoftDevice with Pager"
    )
    parser.add_argument(
        "--file",
        "-f",
        type=Path,
        default=Path("dist/xiao-nrf52840/dev/installer/pager-xiao-installer.uf2"),
        help="installer UF2 (default: dist/xiao-nrf52840/dev/installer/pager-xiao-installer.uf2)",
    )
    parser.add_argument(
        "--volume",
        type=Path,
        help="mounted stock UF2 volume; normally detected automatically",
    )
    parser.add_argument("--timeout", type=float, default=20.0)
    args = parser.parse_args()
    try:
        install(args.file, args.volume, args.timeout)
    except (OSError, ValueError, RuntimeError, usb.core.USBError, usb.core.NoBackendError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
