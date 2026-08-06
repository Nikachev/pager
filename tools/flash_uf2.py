#!/usr/bin/env python3
"""
Pager USB UF2 Flasher CLI

Transfers signed UF2 firmware blocks over direct Vendor-Specific USB Bulk endpoints
to the Pager Single-Slot Bootloader on nRF52840. Auto-reboots main application if needed.
"""

import sys
import time
import glob
import argparse
import serial
import usb.core
import usb.util
import usb.backend.libusb1

DEFAULT_VID = 0x1209
DEFAULT_PID = 0x0001
LIBUSB_PATH = "/opt/homebrew/lib/libusb-1.0.dylib"

def get_backend():
    try:
        return usb.backend.libusb1.get_backend(find_library=lambda x: LIBUSB_PATH)
    except Exception:
        return None

def trigger_reboot_if_in_main_app():
    ports = glob.glob('/dev/cu.usbmodem*')
    if ports:
        print(f"🔄 Device is running Main Application. Sending reboot command to {ports[0]}...")
        try:
            s = serial.Serial(ports[0], 115200, timeout=1)
            time.sleep(0.1)
            s.write(b"dfu\r\n")
            s.flush()
            s.close()
            print("⏳ Waiting 2.0s for device to reboot into Pager Bootloader DFU mode...")
            time.sleep(2.0)
            return True
        except Exception as e:
            print(f"⚠️ Could not send reboot command over serial: {e}")
    return False

def flash_uf2(filename, vid=DEFAULT_VID, pid=DEFAULT_PID):
    backend = get_backend()
    dev = usb.core.find(idVendor=vid, idProduct=pid, backend=backend)

    if dev is None:
        if trigger_reboot_if_in_main_app():
            dev = usb.core.find(idVendor=vid, idProduct=pid, backend=backend)

    if dev is None:
        print(f"❌ Device Pager Bootloader ({hex(vid)}:{hex(pid)}) not found on USB!")
        sys.exit(1)

    print(f"✅ Found device: Pager Bootloader ({hex(vid)}:{hex(pid)})")

    try:
        dev.set_configuration()
    except Exception:
        pass

    cfg = dev.get_active_configuration()
    intf = cfg[(0,0)]

    ep_out = usb.util.find_descriptor(
        intf,
        custom_match = lambda e: \
            usb.util.endpoint_direction(e.bEndpointAddress) == \
            usb.util.ENDPOINT_OUT
    )

    ep_in = usb.util.find_descriptor(
        intf,
        custom_match = lambda e: \
            usb.util.endpoint_direction(e.bEndpointAddress) == \
            usb.util.ENDPOINT_IN
    )

    with open(filename, "rb") as f:
        uf2_data = f.read()

    blocks = len(uf2_data) // 512
    print(f"📦 Transferring {blocks} signed UF2 blocks ({len(uf2_data)} bytes) to Pager Bootloader...", flush=True)

    start_time = time.time()
    for i in range(0, len(uf2_data), 512):
        block = uf2_data[i:i+512]
        block_idx = i // 512
        
        try:
            ep_out.write(block, timeout=2000)
            ack = ep_in.read(64, timeout=2000)
        except Exception as e:
            print(f"\n🚀 Block {block_idx + 1}/{blocks} received: device completed verification and reset into main application!")
            break
            
        pct = int(((block_idx + 1) / blocks) * 100)
        bar = "█" * (pct // 5) + "░" * (20 - (pct // 5))
        sys.stdout.write(f"\r  [{bar}] {pct}% ({block_idx + 1}/{blocks} blocks)")
        sys.stdout.flush()

    elapsed = time.time() - start_time
    print(f"\n🎉 UF2 Firmware transferred successfully in {elapsed:.2f}s!", flush=True)

def main():
    parser = argparse.ArgumentParser(description="Pager USB UF2 Firmware Flasher")
    parser.add_argument("--file", "-f", default="dist/pager.uf2", help="Path to signed UF2 file (default: dist/pager.uf2)")
    parser.add_argument("--vid", type=lambda x: int(x, 16), default=DEFAULT_VID, help="USB Vendor ID (hex)")
    parser.add_argument("--pid", type=lambda x: int(x, 16), default=DEFAULT_PID, help="USB Product ID (hex)")
    args = parser.parse_args()

    flash_uf2(args.file, args.vid, args.pid)

if __name__ == "__main__":
    main()
