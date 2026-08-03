#!/usr/bin/env python3
"""
Print the inactive A/B slot reported by a USB-connected Pager via WebUSB.
"""

import sys
import struct
import zlib

USB_FRAME_MAGIC = b"PGR1"
USB_FRAME_VERSION = 1
USB_FRAME_HEADER_LEN = 16
KIND_COMMAND = 1
OPCODE_GET_INFO = 2


def crc32(data: bytes) -> int:
    return zlib.crc32(data) & 0xFFFFFFFF


def main() -> int:
    try:
        import usb.core
        backend = None
        try:
            import libusb_package
            backend = libusb_package.get_libusb1_backend()
        except Exception:
            pass

        dev = usb.core.find(idVendor=0x1209, idProduct=0x0001, backend=backend)
        if dev is None:
            raise RuntimeError("Pager USB device not found")

        # Claim Vendor Specific interface 4
        import usb.util
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

                    payload = bytes([OPCODE_GET_INFO])
                    frame = struct.pack(
                        "<4sBBIHI",
                        USB_FRAME_MAGIC,
                        USB_FRAME_VERSION,
                        KIND_COMMAND,
                        1,
                        len(payload),
                        crc32(payload),
                    ) + payload

                    ep_out.write(frame)
                    resp = ep_in.read(64, timeout=1000)
                    resp_bytes = bytes(resp)
                    if b"slot=A" in resp_bytes:
                        print("B")
                        return 0
                    elif b"slot=B" in resp_bytes:
                        print("A")
                        return 0
    except Exception as error:
        print(f"cannot determine inactive slot via WebUSB: {error}", file=sys.stderr)
        # Fallback to A if board is not connected
        print("A")
        return 0

    print("A")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
