#!/usr/bin/env python3
"""
Serial DFU flash script for nice!nano v2 (nRF52840) pager firmware.
Streams a signed package over USB CDC-ACM using `update <bytes> <crc32>`.
"""

import sys
import os
import time
import glob
import fcntl
import zlib
import serial

LOCK_FILE = "/tmp/pager_hil_test.lock"


def wait_for_marker(serial_port, markers, timeout):
    """Wait for a firmware status line and return it, or raise on timeout/error."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        line = serial_port.readline().decode("utf-8", errors="replace").strip()
        if not line:
            continue
        print(f"[device] {line}")
        if any(marker in line for marker in markers):
            return line
    raise TimeoutError(f"Timed out waiting for one of: {', '.join(markers)}")

def find_default_port():
    ports = glob.glob("/dev/cu.usbmodem*")
    if not ports:
        raise RuntimeError("No matching /dev/cu.usbmodem* serial port found. Connect the board first.")
    return ports[0]

def main():
    lock_fd = None
    if os.getenv("PAGER_HIL_LOCK_HELD") != "1":
        try:
            lock_fd = open(LOCK_FILE, "w")
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (OSError, IOError):
            print("[-] Error: Another test or flashing process is currently active on the physical device. Aborting.")
            sys.exit(1)

    port = sys.argv[1] if (len(sys.argv) > 1 and sys.argv[1]) else find_default_port()
    bin_path = sys.argv[2] if (len(sys.argv) > 2 and sys.argv[2]) else "dist/pager.signed.pkg"

    if not os.path.exists(bin_path):
        print(f"[-] Error: Firmware binary '{bin_path}' not found. Run 'make build' first.")
        sys.exit(1)

    print(f"[*] Opening serial port: {port}")
    with open(bin_path, "rb") as f:
        binary_data = f.read()

    file_size = len(binary_data)
    print(f"[*] Binary size: {file_size} bytes ({file_size / 1024:.2f} KB)")

    try:
        # A disconnected CDC endpoint can otherwise block `write()` forever
        # on macOS, leaving the destructive HIL suite locked indefinitely.
        s = serial.Serial(port, 115200, timeout=1, write_timeout=5)
        crc32 = zlib.crc32(binary_data)
        cmd = f"update {file_size} {crc32:08x}\n".encode("utf-8")
        print(f"[*] Sending DFU trigger command: {cmd.strip().decode()}")
        s.write(cmd)
        s.flush()
        ready = wait_for_marker(
            s,
            (f"SERIAL_UPDATE:READY:{file_size}:{crc32:08x}", "SERIAL_UPDATE:ERROR_"),
            timeout=8,
        )
        if "READY" not in ready:
            raise RuntimeError(f"Device rejected serial update: {ready}")

        print("[*] Streaming binary chunks (512 bytes)...")
        chunk_size = 512
        written = 0
        for i in range(0, file_size, chunk_size):
            chunk = binary_data[i:i + chunk_size]
            written_now = s.write(chunk)
            if written_now != len(chunk):
                raise RuntimeError(f"Short serial write: {written_now}/{len(chunk)} bytes")
            time.sleep(0.003)
            written += len(chunk)
            progress = (written / file_size) * 100
            print(f"\r[>] Transferred {written}/{file_size} bytes ({progress:.1f}%)", end="", flush=True)

        print()
        s.flush()
        completed = wait_for_marker(
            s,
            ("SERIAL_UPDATE:COMPLETE", "SERIAL_UPDATE:FLASH_ERROR", "SERIAL_UPDATE:CHECKSUM_MISMATCH"),
            timeout=20,
        )
        if "COMPLETE" not in completed:
            raise RuntimeError(f"Device did not commit serial update: {completed}")
        s.close()
        print("[+] Binary transfer verified! Board will self-flash and reboot.")
    except Exception as e:
        print(f"\n[-] Serial DFU error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
