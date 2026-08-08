#!/usr/bin/env python3
import os
import sys
import time
import glob
import serial
import usb.core
import usb.backend.libusb1

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

backend = get_backend()

ports = glob.glob('/dev/cu.usbmodem*') + glob.glob('/dev/ttyACM*')
if not ports:
    print("❌ No CDC serial port (/dev/cu.usbmodem* / /dev/ttyACM*) found!")
    sys.exit(1)

port = ports[0]
print(f"1. Opening serial port {port}...")
try:
    s = serial.Serial(port, 115200, timeout=1)
    time.sleep(0.1)
    print("2. Sending 'dfu\\r\\n' command to main application...")
    s.write(b"dfu\r\n")
    s.flush()
    s.close()
    print("3. Command sent! Waiting 2.5s for device reboot into DFU mode...")
    time.sleep(2.5)
except Exception as e:
    print(f"Serial exception: {e}")

print("4. Inspecting active USB devices & volumes for Pager Bootloader DFU mode...")

# Check mounted volume (common on macOS)
for mount_path in ["/Volumes/PAGER_BOOT", "/Volumes/NICENANO"]:
    if os.path.exists(mount_path):
        print(f"🎉 SUCCESS! Found mounted Pager Bootloader volume at {mount_path}!")
        sys.exit(0)

SUPPORTED_BOOTLOADERS = [(0x239A, 0x0029), (0x1209, 0x0001)]
for vid, pid in SUPPORTED_BOOTLOADERS:
    dev_bootloader = usb.core.find(idVendor=vid, idProduct=pid, backend=backend)
    if dev_bootloader:
        print(f"🎉 SUCCESS! Found Pager Bootloader USB device ({hex(vid)}:{hex(pid)})!")
        sys.exit(0)

dev_app = usb.core.find(idVendor=0x1209, idProduct=0x0002, backend=backend)
if dev_app:
    print("❌ Device is still in Main Application mode (0x1209:0x0002).")
    sys.exit(1)
else:
    print("❓ No Pager USB device found.")
    sys.exit(2)
