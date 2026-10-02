"""Bounded MSC transfer and CSW validation, separate from signed-package policy."""

import errno
import struct

import usb.core
import usb.util


def validate_csw(csw, tag):
    if len(csw) != 13:
        raise RuntimeError("invalid CSW length")
    magic, received_tag, residue, status = struct.unpack("<4sIIB", bytes(csw))
    if magic != b"USBS" or received_tag != tag or residue != 0 or status != 0:
        raise RuntimeError(f"bootloader rejected transfer: CSW={bytes(csw).hex()}")


def send_scsi_write_10(ep_out, ep_in, tag, lba, block_data):
    cdb = struct.pack(">BBIBHB", 0x2A, 0, lba, 0, 1, 0) + b"\x00" * 6
    cbw = struct.pack("<4sIIBBB", b"USBC", tag, len(block_data), 0, 0, 10) + cdb
    if ep_out.write(cbw, timeout=2000) != len(cbw):
        raise RuntimeError("short CBW write")
    if ep_out.write(block_data, timeout=2000) != len(block_data):
        raise RuntimeError("short UF2 write")
    csw = ep_in.read(13, timeout=2000)
    validate_csw(csw, tag)
    return csw


def raw_transfer(device, data, send=send_scsi_write_10):
    claimed = detached = False
    try:
        try:
            if device.is_kernel_driver_active(0):
                device.detach_kernel_driver(0)
                detached = True
        except NotImplementedError:
            pass
        config = device.get_active_configuration()
        if config is None:
            device.set_configuration()
            config = device.get_active_configuration()
        usb.util.claim_interface(device, 0)
        claimed = True
        interface = config[(0, 0)]
        incoming = usb.util.find_descriptor(
            interface,
            custom_match=lambda ep: (
                usb.util.endpoint_direction(ep.bEndpointAddress) == usb.util.ENDPOINT_IN
            ),
        )
        outgoing = usb.util.find_descriptor(
            interface,
            custom_match=lambda ep: (
                usb.util.endpoint_direction(ep.bEndpointAddress) == usb.util.ENDPOINT_OUT
            ),
        )
        if incoming is None or outgoing is None:
            raise RuntimeError("MSC endpoints not found")
        count = len(data) // 512
        for number in range(count):
            try:
                send(
                    outgoing, incoming, number + 1, number, data[number * 512 : (number + 1) * 512]
                )
            except usb.core.USBError as error:
                if number == count - 1 and error.errno in (
                    errno.ENODEV,
                    errno.ETIMEDOUT,
                    errno.EIO,
                ):
                    # Verification below is mandatory; reset alone is not success.
                    return
                raise
    finally:
        if claimed:
            try:
                usb.util.release_interface(device, 0)
            except usb.core.USBError:
                pass  # Device may have reset; dispose still runs.
        if detached:
            try:
                device.attach_kernel_driver(0)
            except usb.core.USBError, NotImplementedError:
                pass  # The original USB identity may no longer exist.
        usb.util.dispose_resources(device)
