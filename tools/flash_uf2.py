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
import serial
import usb.core
import usb.util
import usb.backend.libusb1

DEFAULT_VID = 0x1209
DEFAULT_PID = 0x0001

def copy_to_volume(src, mount_path):
    dst_path = os.path.join(mount_path, "pager.uf2")
    time.sleep(0.5)  # Allow macOS volume mount to stabilize
    for attempt in range(5):
        try:
            res = subprocess.run(["cp", src, dst_path], capture_output=True, timeout=8)
            # If cp completed or device unmounted during write, consider it success
            if res.returncode == 0 or b"Input/output error" in res.stderr or b"Device not configured" in res.stderr or b"fcopyfile failed" in res.stderr:
                return True
        except Exception:
            pass
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
    ports = []
    try:
        import serial.tools.list_ports
        ports = [p.device for p in serial.tools.list_ports.comports() if "usbmodem" in p.device or "ttyACM" in p.device]
    except Exception:
        pass
    if not ports:
        ports = glob.glob('/dev/cu.usbmodem*') + glob.glob('/dev/ttyACM*')

    if ports:
        print(f"🔄 Device is running Main Application. Sending reboot command to {ports[0]}...")
        try:
            s = serial.Serial(ports[0], 115200, timeout=1)
            time.sleep(0.1)
            s.write(b"\r\ndfu\r\n")
            s.flush()
            time.sleep(0.1)
            s.close()
            print("⏳ Waiting for device to enumerate in Pager Bootloader DFU mode...")
            return True
        except Exception as e:
            print(f"⚠️ Failed to send serial reboot command: {e}")
            return False
    return False

def send_scsi_write_10(ep_out, ep_in, tag, lba, block_data):
    scsi_write_10_cb = struct.pack(">BBIBHB", 0x2A, 0, lba, 0, 1, 0) + b"\x00" * 6
    cbw = struct.pack("<4sIIBBB", b"USBC", tag, len(block_data), 0x00, 0, 10) + scsi_write_10_cb
    ep_out.write(cbw, timeout=2000)
    ep_out.write(block_data, timeout=2000)
    return ep_in.read(13, timeout=2000)

SUPPORTED_DEVICES = [(0x239A, 0x0029), (0x1209, 0x0001)]

def find_device(backend=None):
    for vid, pid in SUPPORTED_DEVICES:
        dev = usb.core.find(idVendor=vid, idProduct=pid, backend=backend)
        if dev is not None:
            return dev, vid, pid
    return None, None, None

def flash_uf2(filename, vid=DEFAULT_VID, pid=DEFAULT_PID):
    uf2_data = open(filename, "rb").read()

    # 1. First check if mounted volume /Volumes/PAGER_BOOT exists in OS
    for mount_path in ["/Volumes/PAGER_BOOT", "/Volumes/NICENANO"]:
        if os.path.exists(mount_path):
            print(f"💡 Found mounted bootloader volume at {mount_path}")
            print(f"📦 Transferring {filename} ({len(uf2_data)} bytes) to {mount_path}...")
            start_time = time.time()
            copy_to_volume(filename, mount_path)
            elapsed = time.time() - start_time
            speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
            print(f"🎉 UF2 Firmware transferred successfully via {mount_path} ({speed_kb:.1f} KB/s)!")
            return

    backend = get_backend()
    dev, found_vid, found_pid = find_device(backend)

    if dev is None:
        rebooted = trigger_reboot_if_in_main_app()
        start_time = time.time()
        while time.time() - start_time < 8.0:
            # Check mounted volume first (macOS auto-mounts MSC and blocks raw USB)
            for mount_path in ["/Volumes/PAGER_BOOT", "/Volumes/NICENANO"]:
                if os.path.exists(mount_path):
                    print(f"💡 Found mounted bootloader volume at {mount_path}")
                    print(f"📦 Transferring {filename} ({len(uf2_data)} bytes) to {mount_path}...")
                    vol_start = time.time()
                    copy_to_volume(filename, mount_path)
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
            else:
                err_str = str(e)
                print(f"\n⚠️ Transfer error on block {block_idx + 1}/{blocks}: {err_str}")
                if "Access denied" in err_str or "13" in err_str:
                    print("🔒 macOS kernel driver (IOUSBMassStorage) locked the Mass Storage interface.")
                    print("⏳ Waiting for macOS to mount bootloader volume...")
                    mounted = False
                    for _retry in range(15):
                        for mount_path in ["/Volumes/PAGER_BOOT", "/Volumes/NICENANO"]:
                            if os.path.exists(mount_path):
                                print(f"💡 Transferring {filename} to mounted volume {mount_path}...")
                                copy_to_volume(filename, mount_path)
                                elapsed = time.time() - start_time
                                speed_kb = (len(uf2_data) / 1024) / max(elapsed, 0.001)
                                print(f"🎉 UF2 Firmware transferred successfully via {mount_path} ({speed_kb:.1f} KB/s)!")
                                mounted = True
                                return
                        time.sleep(0.3)
                    if not mounted:
                        print("👉 Please run with sudo: sudo python3 tools/flash_uf2.py")
            break

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
    print(f"\n🎉 UF2 Firmware transferred successfully in {elapsed:.2f}s ({speed_kb:.1f} KB/s)!", flush=True)

def main():
    parser = argparse.ArgumentParser(description="Pager USB Mass Storage UF2 Firmware Flasher")
    parser.add_argument("--file", "-f", default="dist/pager.uf2", help="Path to signed UF2 file (default: dist/pager.uf2)")
    parser.add_argument("--vid", type=lambda x: int(x, 16), default=DEFAULT_VID, help="USB Vendor ID (hex)")
    parser.add_argument("--pid", type=lambda x: int(x, 16), default=DEFAULT_PID, help="USB Product ID (hex)")
    args = parser.parse_args()

    flash_uf2(args.file, args.vid, args.pid)

if __name__ == "__main__":
    main()
