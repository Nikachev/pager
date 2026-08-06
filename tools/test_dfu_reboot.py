#!/usr/bin/env python3
import sys
import time
import glob
import serial
import usb.core
import usb.backend.libusb1

LIBUSB_PATH = "/opt/homebrew/lib/libusb-1.0.dylib"
backend = usb.backend.libusb1.get_backend(find_library=lambda x: LIBUSB_PATH)

ports = glob.glob('/dev/cu.usbmodem*')
if not ports:
    print("❌ No CDC serial port (/dev/cu.usbmodem*) found!")
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
    print("3. Command sent! Waiting 2.0s for device reboot into DFU mode...")
    time.sleep(2.0)
except Exception as e:
    print(f"Serial exception: {e}")

print("4. Inspecting active USB devices for Pager Bootloader (0x1209:0x0001)...")
dev_bootloader = usb.core.find(idVendor=0x1209, idProduct=0x0001, backend=backend)
dev_app = usb.core.find(idVendor=0x1209, idProduct=0x0002, backend=backend)

if dev_bootloader:
    print("🎉 SUCCESS! Pager Bootloader (0x1209:0x0001) is active on USB!")
    sys.exit(0)
elif dev_app:
    print("❌ Device is still in Main Application mode (0x1209:0x0002).")
    sys.exit(1)
else:
    print("❓ No Pager USB device found.")
    sys.exit(2)
