"""Shared constants and helpers for the pager device test-suite.

This module is imported by the unittest-based tests (``test_device.py``) and
can also be reused by the standalone diagnostic/utility scripts under ``tests/``.

Hardware-specific defaults (serial port, USB volume) live here so they are
defined in exactly one place.
"""

import asyncio
import os
import sys
import time
from serial.tools import list_ports


def find_serial_port():
    env_port = os.getenv("SERIAL_PORT") or os.getenv("PORT")
    if env_port:
        return env_port
    requested_serial = os.getenv("PAGER_USB_SERIAL")
    requested_vid = os.getenv("PAGER_USB_VID")
    requested_pid = os.getenv("PAGER_USB_PID")
    ports = []
    for port in list_ports.comports():
        if not port.device.startswith("/dev/cu.usbmodem"):
            continue
        if requested_serial and port.serial_number != requested_serial:
            continue
        if requested_vid and (port.vid is None or port.vid != int(requested_vid, 0)):
            continue
        if requested_pid and (port.pid is None or port.pid != int(requested_pid, 0)):
            continue
        ports.append(port.device)
    ports.sort()
    if not ports:
        raise RuntimeError("No matching USB modem found")
    if len(ports) != 1:
        raise RuntimeError(f"Multiple USB modems match: {', '.join(ports)}; set SERIAL_PORT")
    return ports[0]


DEFAULT_PORT = os.getenv("SERIAL_PORT") or os.getenv("PORT")

# Custom 128-bit GATT service and its characteristics (see ble.rs CustomService).
SERVICE_UUID = "9e7a0001-0b3e-46e8-ad30-7746bad7128a"
LED_CHAR_UUID = "9e7a0002-0b3e-46e8-ad30-7746bad7128a"
STATUS_CHAR_UUID = "9e7a0003-0b3e-46e8-ad30-7746bad7128a"

# Standard HID-over-GATT characteristics (see ble.rs HidService).
HID_INPUT_REPORT_UUID = "00002a4d-0000-1000-8000-00805f9b34fb"
HID_BOOT_INPUT_REPORT_UUID = "00002a22-0000-1000-8000-00805f9b34fb"
HID_PROTOCOL_MODE_UUID = "00002a4e-0000-1000-8000-00805f9b34fb"
HID_REPORT_MAP_UUID = "00002a4b-0000-1000-8000-00805f9b34fb"
HID_INFO_UUID = "00002a4a-0000-1000-8000-00805f9b34fb"
HID_CONTROL_POINT_UUID = "00002a4c-0000-1000-8000-00805f9b34fb"
DIS_SERVICE_UUID = "0000180a-0000-1000-8000-00805f9b34fb"
BATTERY_SERVICE_UUID = "0000180f-0000-1000-8000-00805f9b34fb"

# Subsystem log markers emitted by log_msg!() in the firmware.
LOG_MARKERS = ("BLE", "SERIAL:", "System heartbeat")


def run_async(coro):
    """Run an async coroutine to completion inside synchronous test code."""
    return asyncio.run(coro)


def wait_for_serial_disconnect(port, timeout=10):
    """Poll list_ports until ``port`` is no longer present."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        present = any(p.device == port for p in list_ports.comports())
        if not present:
            return True
        time.sleep(0.1)
    return False


def wait_for_serial_reconnect(port, timeout=30):
    """Poll list_ports until ``port`` reappears and can be opened."""
    import serial
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with serial.Serial(port, 115200, timeout=0.5):
                return True
        except (serial.SerialException, OSError):
            time.sleep(0.2)
    return False


async def find_ble_device(name_prefix="Pager", timeout=8.0):
    """Discover a Pager device over BLE and return its BleakDevice."""
    from bleak import BleakScanner

    service_uuid_lower = SERVICE_UUID.lower()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        res = await BleakScanner.discover(timeout=3.0, return_adv=True)
        for d, adv in res.values():
            name = d.name or adv.local_name
            if name and name.startswith(name_prefix):
                return d
            if adv.service_uuids and any(u.lower() == service_uuid_lower for u in adv.service_uuids):
                return d
        await asyncio.sleep(0.5)
    return None
