#!/usr/bin/env python3
"""
Pager Live CDC-ACM Diagnostic Log Monitor
Connects to Pager's serial log endpoint (/dev/cu.usbmodem*) and streams live logs.
"""

import sys
import time
import glob
import serial
import serial.tools.list_ports

def find_serial_port():
    try:
        ports = [p.device for p in serial.tools.list_ports.comports() if "usbmodem" in p.device or "ttyACM" in p.device]
        if ports:
            return ports[0]
    except Exception:
        pass
    found = glob.glob('/dev/cu.usbmodem*') + glob.glob('/dev/ttyACM*')
    return found[0] if found else None

fn_main = None

def monitor_logs():
    print("==================================================")
    print("     Pager CDC-ACM Live Log Monitor              ")
    print("==================================================")

    port = find_serial_port()
    if not port:
        print("⏳ Waiting for Pager device serial port (/dev/cu.usbmodem*)...")
        while not port:
            time.sleep(0.5)
            port = find_serial_port()

    print(f"✅ Connected to serial port: {port}")
    print("📡 Streaming live diagnostic logs (Ctrl+C to stop)...\n")

    try:
        s = serial.Serial(port, 115200, timeout=1)
        s.write(b"\r\n")
        s.flush()
        while True:
            line = s.readline().decode('utf-8', errors='ignore').strip()
            if line:
                t = time.strftime("%H:%M:%S")
                print(f"\033[90m[{t}]\033[0m {line}")
    except KeyboardInterrupt:
        print("\n👋 Monitor stopped.")
    except Exception as e:
        print(f"\n⚠️ Serial connection error: {e}")

if __name__ == "__main__":
    monitor_logs()
