#!/usr/bin/env python3
"""
Pager USB Mass Storage UF2 Flasher CLI

Transfers signed UF2 firmware blocks over standard USB Mass Storage (SCSI Bulk-Only Transport)
to the Pager Bootloader on nRF52840. Auto-reboots main application if needed.
"""

import sys
import time
import os
import glob
import struct
import argparse
import subprocess
from pathlib import Path
import usb.core
import usb.util
import usb.backend.libusb1

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from pager_tools.usb import PagerUsbClient, find_application

DEFAULT_VID = 0x239A
DEFAULT_PID = 0x0029
APP_VID = 0x1209
APP_PID = 0x0002

def copy_to_volume(src, mount_path):
    dst_path = os.path.join(mount_path, "pager.uf2")
    time.sleep(0.5)  # Allow macOS volume mount to stabilize
    for attempt in range(5):
        try:
            res = subprocess.run(["cp", src, dst_path], capture_output=True, timeout=8)
            if res.returncode == 0:
                return True
            print(f"copy attempt {attempt + 1} failed: {res.stderr.decode(errors='replace').strip()}")
        except (OSError, subprocess.SubprocessError) as error:
            print(f"copy attempt {attempt + 1} failed: {error}")
        # A successful update resets and unmounts the volume before macOS may
        # report cp completion. The caller proves success through GET_INFO.
        if not os.path.exists(mount_path):
            return False
        time.sleep(0.5)
    return False

def get_backend():
    try:
        import libusb_package
        backend = libusb_package.get_libusb1_backend()
        if backend is not None:
            return backend
    except Exception:
        pass

    for path in ["/opt/homebrew/lib/libusb-1.0.dylib", "/usr/local/lib/libusb-1.0.dylib", "/usr/lib/libusb-1.0.dylib"]:
        if os.path.exists(path):
            try:
                backend = usb.backend.libusb1.get_backend(find_library=lambda x: path)
                if backend is not None:
                    return backend
            except Exception:
                pass
    return None

def trigger_reboot_if_in_main_app():
    try:
        backend = get_backend()
        device = find_application(backend=backend, serial=os.getenv("PAGER_USB_SERIAL"))
        if device is None:
            return False
        print("🔄 Device is running the application. Requesting bootloader over WebUSB...")
        with PagerUsbClient(device) as client:
            client.call(bytes([9]))
        print("⏳ Waiting for Pager Bootloader to enumerate...")
        return True
    except (OSError, usb.core.USBError, RuntimeError) as error:
        print(f"⚠️ Failed to request bootloader over WebUSB: {error}")
        return False

def send_scsi_write_10(ep_out, ep_in, tag, lba, block_data):
    scsi_write_10_cb = struct.pack(">BBIBHB", 0x2A, 0, lba, 0, 1, 0) + b"\x00" * 6
    cbw = struct.pack("<4sIIBBB", b"USBC", tag, len(block_data), 0x00, 0, 10) + scsi_write_10_cb
    ep_out.write(cbw, timeout=2000)
    ep_out.write(block_data, timeout=2000)
    return ep_in.read(13, timeout=2000)

SUPPORTED_DEVICES = [(DEFAULT_VID, DEFAULT_PID)]


def wait_for_application(backend=None, timeout=15.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if find_application(backend=backend, serial=os.getenv("PAGER_USB_SERIAL")) is not None:
            return True
        time.sleep(0.25)
    return False


def verify_running_application(backend=None):
    device = find_application(backend=backend, serial=os.getenv("PAGER_USB_SERIAL"))
    if device is None:
        raise RuntimeError("Pager application did not enumerate")
    with PagerUsbClient(device) as client:
        info = client.get_info()
    if "protocol=5" not in info:
        raise RuntimeError(f"unexpected Pager GET_INFO: {info}")
    version_path = REPO_ROOT / "dist" / "version.txt"
    if version_path.exists():
        expected = version_path.read_text(encoding="utf-8").strip()
        if f"version={expected}" not in info:
            raise RuntimeError(f"Pager booted {info}, expected version={expected}")
    print(f"✅ Verified running application: {info}")


def copy_and_verify(filename, mount_path, backend=None):
    copy_reported_success = copy_to_volume(filename, mount_path)
    if not wait_for_application(backend):
        if copy_reported_success:
            raise RuntimeError("UF2 copied, but Pager application did not enumerate")
        raise RuntimeError("UF2 copy failed and Pager application did not enumerate")
    verify_running_application(backend)
    if not copy_reported_success:
        print("ℹ️ macOS ended the copy while Pager reset; GET_INFO confirmed success.")

def find_device(backend=None):
    for vid, pid in SUPPORTED_DEVICES:
        dev = usb.core.find(idVendor=vid, idProduct=pid, backend=backend)
        if dev is not None:
            return dev, vid, pid
    return None, None, None

def flash_uf2(filename, vid=DEFAULT_VID, pid=DEFAULT_PID):
    uf2_data = open(filename, "rb").read()

    # 1. First check if mounted volume /Volumes/PAGER_BOOT exists in OS
    for mount_path in ["/Volumes/PAGER_BOOT"]:
        if os.path.exists(mount_path):
            print(f"💡 Found mounted bootloader volume at {mount_path}")
            print(f"📦 Transferring {filename} ({len(uf2_data)} bytes) to {mount_path}...")
            start_time = time.time()
            copy_and_verify(filename, mount_path, get_backend())
            elapsed = time.time() - start_time
            speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
            print(f"🎉 UF2 Firmware transferred successfully via {mount_path} ({speed_kb:.1f} KB/s)!")
            return

    backend = get_backend()
    dev, found_vid, found_pid = find_device(backend)

    if dev is None:
        trigger_reboot_if_in_main_app()
        start_time = time.time()
        while time.time() - start_time < 8.0:
            # Check mounted volume first (macOS auto-mounts MSC and blocks raw USB)
            for mount_path in ["/Volumes/PAGER_BOOT"]:
                if os.path.exists(mount_path):
                    print(f"💡 Found mounted bootloader volume at {mount_path}")
                    print(f"📦 Transferring {filename} ({len(uf2_data)} bytes) to {mount_path}...")
                    vol_start = time.time()
                    copy_and_verify(filename, mount_path, get_backend())
                    elapsed = time.time() - vol_start
                    speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
                    print(f"🎉 UF2 Firmware transferred successfully via {mount_path} ({speed_kb:.1f} KB/s)!")
                    return
            dev, found_vid, found_pid = find_device(backend)
            if dev is not None:
                break
            time.sleep(0.3)

    if dev is None:
        print(f"❌ Device Pager Bootloader not found on USB!")
        sys.exit(1)

    print(f"✅ Found device: Pager Bootloader USB Mass Storage ({hex(found_vid)}:{hex(found_pid)})")

    try:
        if dev.is_kernel_driver_active(0):
            dev.detach_kernel_driver(0)
    except Exception:
        pass

    try:
        dev.set_configuration()
    except Exception:
        pass

    try:
        usb.util.claim_interface(dev, 0)
    except Exception:
        pass

    cfg = dev.get_active_configuration()
    intf = cfg[(0,0)]

    ep_out = usb.util.find_descriptor(
        intf,
        custom_match = lambda e: usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_OUT
    )
    ep_in = usb.util.find_descriptor(
        intf,
        custom_match = lambda e: usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_IN
    )

    with open(filename, "rb") as f:
        uf2_data = f.read()

    blocks = len(uf2_data) // 512
    print(f"📦 Transferring {blocks} signed UF2 blocks ({len(uf2_data)} bytes) via USB Mass Storage SCSI...", flush=True)

    start_time = time.time()
    for i in range(0, len(uf2_data), 512):
        block = uf2_data[i:i+512]
        block_idx = i // 512
        try:
            csw = send_scsi_write_10(ep_out, ep_in, block_idx + 1, block_idx, block)
        except Exception as e:
            if block_idx + 1 >= blocks:
                print(f"\n🚀 Block {block_idx + 1}/{blocks} received: device completed verification and reset into main application!")
                if not wait_for_application(backend):
                    raise RuntimeError(
                        "final UF2 block sent, but Pager application did not enumerate"
                    ) from e
                verify_running_application(backend)
                return
            else:
                err_str = str(e)
                print(f"\n⚠️ Transfer error on block {block_idx + 1}/{blocks}: {err_str}")
                if "Access denied" in err_str or "13" in err_str:
                    print("🔒 macOS USB interface locked by IOUSBMassStorage or Chrome WebUSB.")
                    print("⏳ Searching for mounted bootloader volume in /Volumes...")
                    for _retry in range(20):
                        vols = glob.glob("/Volumes/PAGER_BOOT")
                        for mount_path in vols:
                            if os.path.exists(mount_path):
                                print(f"💡 Transferring {filename} to mounted volume {mount_path}...")
                                copy_and_verify(filename, mount_path, get_backend())
                                elapsed = time.time() - start_time
                                speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
                                print(f"🎉 UF2 Firmware transferred successfully via {mount_path} ({speed_kb:.1f} KB/s)!")
                                return
                        time.sleep(0.3)
                    print("👉 Close the Chrome WebUSB tab and retry.")
                raise RuntimeError(f"UF2 transfer failed at block {block_idx + 1}/{blocks}: {e}") from e

        csw = bytes(csw)
        if len(csw) != 13 or csw[:4] != b"USBS" or csw[12] != 0:
            raise RuntimeError(f"bootloader rejected UF2 block {block_idx + 1}: CSW={csw.hex()}")

        pct = int(((block_idx + 1) / blocks) * 100)
        elapsed = time.time() - start_time
        speed_kb = ((block_idx + 1) * 0.5) / max(elapsed, 0.001)
        bar = "█" * (pct // 5) + "░" * (20 - (pct // 5))
        sys.stdout.write(f"\r  [{bar}] {pct}% ({block_idx + 1}/{blocks} blocks, {speed_kb:.1f} KB/s)")
        sys.stdout.flush()

    try:
        usb.util.release_interface(dev, 0)
    except Exception:
        pass

    elapsed = time.time() - start_time
    speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
    if not wait_for_application(backend):
        raise RuntimeError("bootloader accepted UF2, but Pager application did not enumerate")
    verify_running_application(backend)
    print(f"\n🎉 UF2 Firmware transferred and Pager enumerated in {elapsed:.2f}s ({speed_kb:.1f} KB/s)!", flush=True)

def flash_uf2_bytes(uf2_data: bytes, vid=DEFAULT_VID, pid=DEFAULT_PID):
    if len(uf2_data) < 512 or len(uf2_data) % 512 != 0:
        raise ValueError("Invalid UF2 payload length: must be a multiple of 512 bytes")
    magic0, magic1, magic_end = struct.unpack("<III", uf2_data[:8] + uf2_data[508:512])
    if magic0 != 0x0A324655 or magic1 != 0x9E5D5157 or magic_end != 0x0AB16F30:
        raise ValueError("Invalid UF2 block magic numbers")
    import tempfile
    with tempfile.NamedTemporaryFile(suffix=".uf2", delete=False) as f:
        f.write(uf2_data)
        tmp_path = f.name
    try:
        flash_uf2(tmp_path, vid, pid)
    finally:
        if os.path.exists(tmp_path):
            os.remove(tmp_path)

def main():
    parser = argparse.ArgumentParser(description="Pager USB Mass Storage UF2 Firmware Flasher")
    parser.add_argument("--file", "-f", default="dist/pager.uf2", help="Path to signed UF2 file (default: dist/pager.uf2)")
    parser.add_argument("--vid", type=lambda x: int(x, 16), default=DEFAULT_VID, help="USB Vendor ID (hex)")
    parser.add_argument("--pid", type=lambda x: int(x, 16), default=DEFAULT_PID, help="USB Product ID (hex)")
    args = parser.parse_args()

    flash_uf2(args.file, args.vid, args.pid)

if __name__ == "__main__":
    main()
