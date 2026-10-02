"""Shared constants and helpers for the pager device test-suite.

This module is imported by the unittest-based tests (``test_device.py``) and
can also be reused by the standalone diagnostic/utility scripts under ``tests/``.

Hardware-specific defaults (serial port, USB volume) live here so they are
defined in exactly one place.
"""

import asyncio
import os
import time
from serial.tools import list_ports


DEFAULT_PORT = os.getenv("SERIAL_PORT") or os.getenv("PORT")

# Pager advertises only the standard HID and Battery services.
SERVICE_UUID = "00001812-0000-1000-8000-00805f9b34fb"

# Standard HID-over-GATT characteristics (see ble.rs HidService).
HID_INPUT_REPORT_UUID = "00002a4d-0000-1000-8000-00805f9b34fb"
HID_BOOT_INPUT_REPORT_UUID = "00002a22-0000-1000-8000-00805f9b34fb"
HID_PROTOCOL_MODE_UUID = "00002a4e-0000-1000-8000-00805f9b34fb"
HID_REPORT_MAP_UUID = "00002a4b-0000-1000-8000-00805f9b34fb"
HID_INFO_UUID = "00002a4a-0000-1000-8000-00805f9b34fb"
HID_CONTROL_POINT_UUID = "00002a4c-0000-1000-8000-00805f9b34fb"
BATTERY_SERVICE_UUID = "0000180f-0000-1000-8000-00805f9b34fb"
BATTERY_LEVEL_UUID = "00002a19-0000-1000-8000-00805f9b34fb"

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
        except serial.SerialException, OSError:
            time.sleep(0.2)
    return False


async def find_ble_device(name_prefix="Pager", timeout=8.0):
    """Discover a Pager device over BLE and return its BleakDevice."""
    from bleak import BleakScanner

    service_uuid_lower = SERVICE_UUID.lower()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        res = await BleakScanner.discover(timeout=1.5, return_adv=True)
        for d, adv in res.values():
            name = d.name or adv.local_name
            if name and name.startswith(name_prefix):
                return d
            if adv.service_uuids and any(
                u.lower() == service_uuid_lower for u in adv.service_uuids
            ):
                return d
        await asyncio.sleep(0.2)
    return None
