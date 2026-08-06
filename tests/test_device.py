import os
import time
import subprocess
import sys
import serial
import asyncio
import pytest
from pathlib import Path
from bleak import BleakClient

from common import (
    find_serial_port,
    run_async,
    wait_for_serial_reconnect,
    wait_for_serial_disconnect,
    find_ble_device,
    SERVICE_UUID,
    LED_CHAR_UUID,
    STATUS_CHAR_UUID,
    HID_INPUT_REPORT_UUID,
    HID_BOOT_INPUT_REPORT_UUID,
    HID_PROTOCOL_MODE_UUID,
    HID_REPORT_MAP_UUID,
    HID_INFO_UUID,
    DIS_SERVICE_UUID,
    BATTERY_SERVICE_UUID,
    LOG_MARKERS,
    DEFAULT_PORT,
)

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


@pytest.fixture(scope="session")
def serial_port():
    for _ in range(10):
        try:
            port = find_serial_port()
            if port:
                return port
        except Exception:
            pass
        time.sleep(0.5)
    return DEFAULT_PORT


# ---------------------------------------------------------------------------
# BLE Functionality Tests
# ---------------------------------------------------------------------------

def _trigger_webusb_disconnect():
    try:
        import usb.core, usb.util, struct, zlib, libusb_package
        backend = libusb_package.get_libusb1_backend()
        dev = usb.core.find(idVendor=0x1209, idProduct=0x0002, backend=backend)
        if dev:
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
                        payload = bytes([6])  # DISCONNECT opcode
                        frame = struct.pack(
                            "<4sBBIHI",
                            b"PGR1",
                            1,
                            1,
                            1,
                            len(payload),
                            zlib.crc32(payload) & 0xFFFFFFFF,
                        ) + payload
                        ep_out.write(frame)
                        usb.util.release_interface(dev, intf.bInterfaceNumber)
    except Exception:
        pass


async def find_hil_ble_device(retries=3):
    for attempt in range(retries):
        dev = await find_ble_device("Pager")
        if dev:
            return dev
        _trigger_webusb_disconnect()
        await asyncio.sleep(1.0)
    raise RuntimeError("Could not find BLE device 'Pager'")


@pytest.mark.ble
def test_ble_functionality():
    """Connect to Pager over BLE, read Status, and write LED states."""
    print("\n--- Running BLE Functionality Test ---")

    async def run_ble_test():
        device = await find_hil_ble_device()
        print(f"Found Pager BLE Device: {device.name} [{device.address}]...")

        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"
            print("Connected to BLE GATT server!")

            available_uuids = [c.uuid.lower() for s in client.services for c in s.characteristics]
            if STATUS_CHAR_UUID.lower() not in available_uuids:
                print("Custom service characteristics not exposed in current macOS GATT session")
                return

            status = await client.read_gatt_char(STATUS_CHAR_UUID)
            print(f"Read Status characteristic: {bytes(status)}")
            assert len(status) == 1, f"Expected 1 byte, got {len(status)}"

            print("Writing LED mode 1 (High)...")
            await client.write_gatt_char(LED_CHAR_UUID, bytearray([0x01]))
            await asyncio.sleep(0.5)

            print("Writing LED mode 0 (Blink)...")
            await client.write_gatt_char(LED_CHAR_UUID, bytearray([0x00]))
            await asyncio.sleep(0.5)

            print("BLE test completed successfully!")

    try:
        run_async(run_ble_test())
    except Exception as e:
        if "Could not find BLE device" in str(e) or "Bluetooth" in str(e):
            pytest.skip(f"BLE test skipped: {e}")
        raise


# ---------------------------------------------------------------------------
# CDC Serial Logs & DFU Reboot Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_serial_logs():
    """Test retrieving live logs from CDC-ACM serial endpoint"""
    print("\n--- Running Serial Logs Test ---")
    port = None
    for _ in range(10):
        try:
            port = find_serial_port()
            if port:
                break
        except Exception:
            pass
        time.sleep(0.5)
    assert port, "Serial port not found"
    try:
        s = serial.Serial(port, 115200, timeout=2)
        s.write(b"\r\n")
        s.flush()
        lines = []
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            line = s.readline().decode('utf-8', errors='ignore').strip()
            if line:
                lines.append(line)
                if len(lines) >= 3:
                    break
        s.close()
        full_text = "\n".join(lines)
        print("Serial logs received:")
        print(full_text)
        assert len(lines) > 0, "No logs received from CDC-ACM port"
    except Exception as e:
        pytest.fail(f"Serial port failed: {e}")


@pytest.mark.dfu
def test_dfu_reboot_command(serial_port):
    """Test software reboot command ('dfu') into Pager Bootloader DFU mode"""
    print("\n--- Running DFU Reboot Command Test ---")
    res = subprocess.run(
        [sys.executable, os.path.join(_REPO_ROOT, "tools", "test_dfu_reboot.py")],
        capture_output=True,
        text=True,
        cwd=_REPO_ROOT,
    )
    assert res.returncode == 0, f"DFU Reboot failed: {res.stdout}\n{res.stderr}"
    print("Device successfully rebooted into Pager Bootloader DFU mode!")


@pytest.mark.dfu
def test_uf2_flashing():
    """Test UF2 firmware flashing to Pager Bootloader"""
    print("\n--- Running UF2 Flashing Test ---")
    uf2_file = os.path.join(_REPO_ROOT, "dist", "pager.uf2")
    assert os.path.exists(uf2_file), f"UF2 file not found: {uf2_file}"

    res = subprocess.run(
        [sys.executable, os.path.join(_REPO_ROOT, "tools", "flash_uf2.py"), "--file", uf2_file],
        capture_output=True,
        text=True,
        cwd=_REPO_ROOT,
    )
    assert res.returncode == 0, f"UF2 Flashing failed: {res.stdout}\n{res.stderr}"
    print("UF2 Firmware transferred successfully and application booted!")


@pytest.mark.ble
def test_visible_gatt_metadata_and_hid_when_exposed():
    """Verify visible metadata and HID details when CoreBluetooth exposes them."""
    print("\n--- Running BLE HID metadata/report Test ---")

    async def run_services_test():
        device = await find_hil_ble_device()
        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"

            services = client.services
            uuids = [s.uuid.lower() for s in services]

            if DIS_SERVICE_UUID.lower() not in uuids:
                print("DIS service (0x180A) not exposed in current macOS GATT session")
                return

            manufacturer = await client.read_gatt_char("00002a29-0000-1000-8000-00805f9b34fb")
            assert manufacturer.decode("utf-8", "ignore") == "Nikachev"
            model = await client.read_gatt_char("00002a24-0000-1000-8000-00805f9b34fb")
            assert model.decode("utf-8", "ignore") == "Pager-nRF52840"

            assert BATTERY_SERVICE_UUID.lower() in uuids, "Battery service (0x180F) not found"

            available_characteristics = {
                characteristic.uuid.lower()
                for service in services
                for characteristic in service.characteristics
            }
            if HID_REPORT_MAP_UUID.lower() not in available_characteristics:
                return
            report_map = await client.read_gatt_char(HID_REPORT_MAP_UUID)
            assert len(report_map) > 0, "HID Report Map is empty"
            assert bytes(report_map[:6]) == bytes([0x05, 0x01, 0x09, 0x06, 0xA1, 0x01])

            mode = await client.read_gatt_char(HID_PROTOCOL_MODE_UUID)
            assert mode[0] == 1, "Default HID protocol mode should be 1 (Report)"
            await client.write_gatt_char(HID_PROTOCOL_MODE_UUID, bytearray([0x00]))
            mode = await client.read_gatt_char(HID_PROTOCOL_MODE_UUID)
            assert mode[0] == 0, "HID protocol mode write to 0 was not reflected"
            await client.write_gatt_char(HID_PROTOCOL_MODE_UUID, bytearray([0x01]))

            for mode_byte in (0x00, 0x01, 0x02):
                await client.write_gatt_char(LED_CHAR_UUID, bytearray([mode_byte]))

    try:
        run_async(run_services_test())
    except Exception as e:
        if "Could not find BLE device" in str(e) or "Bluetooth" in str(e):
            pytest.skip(f"BLE test skipped: {e}")
        raise
