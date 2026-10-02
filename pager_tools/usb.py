"""Strict PyUSB discovery and request/response client for Pager."""

import math
import time
import usb.core
import usb.util

from .protocol import FrameKind, ProtocolError, encode_frame, read_frame, resynchronize

from .protocol_spec import APPLICATION_VID, APPLICATION_PID

APP_VID = APPLICATION_VID
APP_PID = APPLICATION_PID


def normalize_serial(serial):
    return (serial or "").strip().upper()


def parse_info(text, separator=";", delimiter="="):
    fields = {}
    for item in text.split(separator):
        if delimiter not in item:
            continue
        name, value = (part.strip() for part in item.split(delimiter, 1))
        if name in fields:
            raise ProtocolError(f"duplicate metadata field: {name}")
        fields[name] = value
    return fields


def find_application(backend=None, serial=None):
    devices = list(
        usb.core.find(find_all=True, idVendor=APP_VID, idProduct=APP_PID, backend=backend) or []
    )
    if serial:
        devices = [
            device
            for device in devices
            if normalize_serial(device.serial_number) == normalize_serial(serial)
        ]
    if len(devices) > 1:
        raise ProtocolError("multiple Pager devices found; set PAGER_USB_SERIAL")
    return devices[0] if devices else None


class PagerUsbClient:
    def __init__(self, device):
        self.device = device
        self.interface = None
        self.endpoint_in = None
        self.endpoint_out = None
        self.buffer = b""
        self.request_id = 1
        self.claimed = False
        self.detached = False

    def __enter__(self):
        if self.device.get_active_configuration() is None:
            self.device.set_configuration()
        config = self.device.get_active_configuration()
        for interface in config:
            if interface.bInterfaceClass != 0xFF:
                continue
            incoming = usb.util.find_descriptor(
                interface,
                custom_match=lambda endpoint: (
                    usb.util.endpoint_direction(endpoint.bEndpointAddress) == usb.util.ENDPOINT_IN
                ),
            )
            outgoing = usb.util.find_descriptor(
                interface,
                custom_match=lambda endpoint: (
                    usb.util.endpoint_direction(endpoint.bEndpointAddress) == usb.util.ENDPOINT_OUT
                ),
            )
            if incoming is not None and outgoing is not None:
                self.interface = interface.bInterfaceNumber
                self.endpoint_in = incoming
                self.endpoint_out = outgoing
                break
        if self.interface is None:
            raise ProtocolError("Pager WebUSB interface not found")
        try:
            try:
                if self.device.is_kernel_driver_active(self.interface):
                    self.device.detach_kernel_driver(self.interface)
                    self.detached = True
            except NotImplementedError:
                pass
            usb.util.claim_interface(self.device, self.interface)
            self.claimed = True
        except usb.core.USBError, OSError:
            self.close()
            raise
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        try:
            if self.claimed:
                try:
                    usb.util.release_interface(self.device, self.interface)
                except usb.core.USBError:
                    pass  # Reset/disconnect can invalidate the interface.
                self.claimed = False
        finally:
            try:
                if self.detached:
                    try:
                        self.device.attach_kernel_driver(self.interface)
                    except usb.core.USBError, NotImplementedError:
                        pass
                    self.detached = False
            finally:
                usb.util.dispose_resources(self.device)

    def call(self, payload: bytes, timeout_ms=5000) -> bytes:
        if timeout_ms <= 0:
            raise ValueError("timeout_ms must be positive")
        deadline = time.monotonic() + timeout_ms / 1000
        request_id = self.request_id
        self.request_id = (self.request_id + 1) & 0xFFFFFFFF or 1
        written = self.endpoint_out.write(
            encode_frame(FrameKind.COMMAND, request_id, payload), timeout=timeout_ms
        )
        if written != len(payload) + 16:
            raise ProtocolError("short Pager USB write")
        while time.monotonic() < deadline:
            parsed = read_frame(self.buffer)
            if parsed is None:
                self.buffer = resynchronize(self.buffer)
                remaining_ms = max(1, math.ceil((deadline - time.monotonic()) * 1000))
                try:
                    packet = self.endpoint_in.read(64, timeout=remaining_ms)
                except usb.core.USBTimeoutError as error:
                    raise ProtocolError("Pager command timeout") from error
                self.buffer += bytes(packet)
                continue
            (kind, received_id, body), self.buffer = parsed
            if kind == FrameKind.EVENT:
                continue
            if received_id != request_id:
                raise ProtocolError(f"unexpected response id {received_id}, expected {request_id}")
            if kind == FrameKind.ERROR:
                code = body[0] if body else "unknown"
                raise ProtocolError(f"Pager command error {code}")
            if kind != FrameKind.RESPONSE:
                raise ProtocolError(f"unexpected response kind {kind}")
            return body
        raise ProtocolError("Pager command timeout")

    def get_info(self) -> str:
        return self.call(bytes([2])).decode("utf-8")
