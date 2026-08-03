#!/usr/bin/env python3
"""
WebUSB DFU flash script for the Pager (nRF52840) firmware.
Streams a signed package over WebUSB bulk endpoints using protocol v1.
"""

import sys
import os
import time
import struct
import zlib
import fcntl

LOCK_FILE = "/tmp/pager_hil_test.lock"

# Protocol v1 Constants
USB_FRAME_MAGIC = b"PGR1"
USB_FRAME_VERSION = 1
USB_FRAME_HEADER_LEN = 16
USB_MAX_PAYLOAD = 512

KIND_COMMAND = 1
KIND_RESPONSE = 2
KIND_EVENT = 3
KIND_DFU_DATA = 4
KIND_ERROR = 5

OPCODE_PING = 1
OPCODE_GET_INFO = 2
OPCODE_GET_KEYBOARD_STATE = 3
OPCODE_DFU_BEGIN = 9
OPCODE_DFU_COMMIT = 10
OPCODE_DFU_ABORT = 11

ERR_BAD_REQUEST = 1
ERR_UNSUPPORTED_COMMAND = 2
ERR_BUSY = 3
ERR_DFU = 4


def crc32(data: bytes) -> int:
    return zlib.crc32(data) & 0xFFFFFFFF


def encode_usb_frame(kind: int, request_id: int, payload: bytes) -> bytes:
    if len(payload) > USB_MAX_PAYLOAD:
        raise ValueError("Payload size exceeds maximum allowed")
    checksum = crc32(payload)
    return struct.pack(
        "<4sBBIHI",
        USB_FRAME_MAGIC,
        USB_FRAME_VERSION,
        kind,
        request_id,
        len(payload),
        checksum,
    ) + payload


def parse_usb_frame(frame: bytes):
    if len(frame) < USB_FRAME_HEADER_LEN:
        raise ValueError("Frame too short")
    magic, version, kind, request_id, payload_len, expected_crc = struct.unpack(
        "<4sBBIHI", frame[:16]
    )
    if magic != USB_FRAME_MAGIC or version != USB_FRAME_VERSION:
        raise ValueError("Invalid frame header magic or version")
    payload = frame[USB_FRAME_HEADER_LEN:USB_FRAME_HEADER_LEN + payload_len]
    if len(payload) != payload_len:
        raise ValueError("Payload length mismatch")
    if crc32(payload) != expected_crc:
        raise ValueError("CRC32 mismatch")
    return kind, request_id, payload


def find_webusb_device():
    import usb.core
    backend = None
    try:
        import libusb_package
        backend = libusb_package.get_libusb1_backend()
    except Exception:
        pass

    dev = usb.core.find(idVendor=0x1209, idProduct=0x0001, backend=backend)
    if dev is None:
        raise RuntimeError("Pager device (VID: 0x1209, PID: 0x0001) not found.")
    return dev


def claim_webusb_interface(dev):
    import usb.util
    # Find vendor-specific interface (class 255)
    for cfg in dev:
        for intf in cfg:
            if intf.bInterfaceClass == 0xFF:
                try:
                    if dev.is_kernel_driver_active(intf.bInterfaceNumber):
                        dev.detach_kernel_driver(intf.bInterfaceNumber)
                except Exception:
                    pass
                usb.util.claim_interface(dev, intf.bInterfaceNumber)
                ep_out = usb.util.find_descriptor(
                    intf,
                    custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
                    == usb.util.ENDPOINT_OUT,
                )
                ep_in = usb.util.find_descriptor(
                    intf,
                    custom_match=lambda e: usb.util.endpoint_direction(e.bEndpointAddress)
                    == usb.util.ENDPOINT_IN,
                )
                return intf.bInterfaceNumber, ep_out, ep_in
    raise RuntimeError("Vendor-specific WebUSB interface (class 0xFF) not found.")


class WebUsbClient:
    def __init__(self, dev, intf_num, ep_out, ep_in):
        self.dev = dev
        self.intf_num = intf_num
        self.ep_out = ep_out
        self.ep_in = ep_in
        self.request_id = 1
        self.rx_buf = bytearray()

    def recv_frame(self, timeout=3000):
        start = time.monotonic()
        while len(self.rx_buf) < USB_FRAME_HEADER_LEN:
            if (time.monotonic() - start) * 1000 > timeout:
                raise TimeoutError("Timeout waiting for frame header")
            try:
                data = self.ep_in.read(64, timeout=500)
                self.rx_buf.extend(data)
            except Exception:
                time.sleep(0.01)

        payload_len = struct.unpack("<H", self.rx_buf[10:12])[0]
        total_len = USB_FRAME_HEADER_LEN + payload_len

        while len(self.rx_buf) < total_len:
            if (time.monotonic() - start) * 1000 > timeout:
                raise TimeoutError("Timeout waiting for complete frame payload")
            try:
                data = self.ep_in.read(64, timeout=500)
                self.rx_buf.extend(data)
            except Exception:
                time.sleep(0.01)

        frame = bytes(self.rx_buf[:total_len])
        del self.rx_buf[:total_len]
        return parse_usb_frame(frame)

    def call(self, kind: int, payload: bytes, timeout=1000):
        req_id = self.request_id
        self.request_id += 1
        frame = encode_usb_frame(kind, req_id, payload)

        for retry in range(200):
            self.ep_out.write(frame)
            busy = False
            for _ in range(8):
                try:
                    resp_kind, resp_id, resp_payload = self.recv_frame(timeout=timeout)
                except TimeoutError:
                    busy = True
                    break
                if resp_id != req_id:
                    continue
                if resp_kind == KIND_ERROR:
                    err_code = resp_payload[0] if resp_payload else 0
                    if err_code == ERR_BUSY:
                        busy = True
                        break
                    raise RuntimeError(f"Device returned error code: {err_code}")
                return resp_payload
            if not busy:
                busy = True
            time.sleep(0.02)
        raise RuntimeError("Device remained busy during transaction")


def main():
    lock_fd = None
    if os.getenv("PAGER_HIL_LOCK_HELD") != "1":
        try:
            lock_fd = open(LOCK_FILE, "w")
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (OSError, IOError):
            print("[-] Error: Another test or flashing process is active. Aborting.")
            sys.exit(1)

    bin_path = sys.argv[1] if (len(sys.argv) > 1 and sys.argv[1]) else "dist/pager-A.signed.pkg"

    if not os.path.exists(bin_path):
        print(f"[-] Error: Firmware package '{bin_path}' not found.")
        sys.exit(1)

    with open(bin_path, "rb") as f:
        package_data = f.read()

    file_size = len(package_data)
    print(f"[*] Package size: {file_size} bytes ({file_size / 1024:.2f} KB)")

    try:
        dev = find_webusb_device()
        intf_num, ep_out, ep_in = claim_webusb_interface(dev)
        client = WebUsbClient(dev, intf_num, ep_out, ep_in)

        # Test PING
        resp = client.call(KIND_COMMAND, bytes([OPCODE_PING]))
        if resp != b"PONG":
            raise RuntimeError(f"Unexpected PING response: {resp}")
        print("[+] WebUSB device ping successful!")

        # Start DFU_BEGIN
        package_crc = crc32(package_data)
        begin_payload = bytes([OPCODE_DFU_BEGIN]) + struct.pack("<II", file_size, package_crc)
        print(f"[*] Initiating WebUSB DFU ({file_size} bytes, CRC: {package_crc:08x})...")
        resp = client.call(KIND_COMMAND, begin_payload, timeout=5000)
        accepted_offset = struct.unpack("<I", resp)[0]
        if accepted_offset != 0:
            raise RuntimeError(f"Unexpected initial offset: {accepted_offset}")

        chunk_size = 508
        offset = 0
        while offset < file_size:
            chunk = package_data[offset:offset + chunk_size]
            data_payload = struct.pack("<I", offset) + chunk
            resp = client.call(KIND_DFU_DATA, data_payload)
            next_offset = struct.unpack("<I", resp)[0]
            if next_offset <= offset:
                raise RuntimeError(f"DFU offset stalled at {offset}")
            offset = next_offset
            progress = (offset / file_size) * 100
            print(f"\r[>] Streamed {offset}/{file_size} bytes ({progress:.1f}%)", end="", flush=True)

        print("\n[*] Committing WebUSB DFU update...")
        client.call(KIND_COMMAND, bytes([OPCODE_DFU_COMMIT]))
        print("[+] WebUSB DFU update committed successfully! Device is restarting.")

    except Exception as e:
        print(f"\n[-] WebUSB DFU error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
