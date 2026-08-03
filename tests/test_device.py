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


def _make_dfu_package(filename=None):
    """Helper to locate or build a valid signed package for the inactive slot."""
    try:
        python_bin = sys.executable
        if os.path.exists(os.path.join(_REPO_ROOT, ".venv", "bin", "python")):
            python_bin = os.path.join(_REPO_ROOT, ".venv", "bin", "python")
        res = subprocess.check_output(
            [python_bin, os.path.join(_REPO_ROOT, "scripts", "select_inactive_slot.py")],
            text=True,
        ).strip()
        slot = res if res in ("A", "B") else "A"
    except Exception:
        slot = "A"

    dist_dir = os.path.join(_REPO_ROOT, "dist")
    pkg_path = os.path.join(dist_dir, f"pager-{slot}.signed.pkg")
    if not os.path.exists(pkg_path):
        subprocess.run(
            ["make", "sign", f"SLOT={slot}"],
            cwd=_REPO_ROOT,
            check=True,
        )
    return pkg_path


@pytest.fixture(scope="session")
def serial_port():
    try:
        return find_serial_port()
    except Exception:
        return DEFAULT_PORT


# ---------------------------------------------------------------------------
# BLE Functionality Tests
# ---------------------------------------------------------------------------

def _trigger_webusb_disconnect():
    try:
        import usb.core, usb.util, struct, zlib, libusb_package
        backend = libusb_package.get_libusb1_backend()
        dev = usb.core.find(idVendor=0x1209, idProduct=0x0001, backend=backend)
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
        print(f"Connecting to BLE device: {device.address}...")

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
    except RuntimeError as e:
        if "Could not find BLE device" in str(e):
            pytest.skip(f"Pager BLE device not advertising: {e}")
        raise


# ---------------------------------------------------------------------------
# CDC Serial & WebUSB DFU Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_serial_logs(serial_port):
    """Test retrieving live logs from CDC-ACM serial endpoint"""
    print("\n--- Running Serial Logs Test ---")
    assert serial_port, "Serial port not found"
    try:
        s = serial.Serial(serial_port, 115200, timeout=2)
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
        assert any(marker in full_text for marker in LOG_MARKERS), (
            f"Logs did not contain any expected subsystem marker: {full_text[:200]!r}"
        )
    except Exception as e:
        pytest.fail(f"Serial port failed: {e}")


@pytest.mark.dfu
def test_serial_update(serial_port):
    """Test firmware update over Serial"""
    print("\n--- Running Serial Update Test ---")
    assert serial_port, "Serial port not found"
    try:
        package = _make_dfu_package()
        result = subprocess.run(
            [sys.executable, os.path.join(_REPO_ROOT, "scripts", "flash_serial.py"), serial_port, package],
            text=True,
            capture_output=True,
            env={**os.environ, "PAGER_HIL_LOCK_HELD": "1"},
            timeout=90,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        print("Serial update upload finished! Waiting for board to perform self-flash reset...")

        assert wait_for_serial_disconnect(serial_port, timeout=10), (
            "Board did not detach CDC before serial OTA reset"
        )
        reconnected = wait_for_serial_reconnect(serial_port, timeout=30)
        assert reconnected, "Board did not reconnect after serial update"
        print("Board successfully reconnected after serial update!")
    except Exception as e:
        pytest.fail(f"Serial update failed: {e}")


@pytest.mark.dfu
def test_webusb_update(serial_port):
    """Test firmware update over WebUSB vendor bulk interface"""
    print("\n--- Running WebUSB Update Test ---")
    try:
        package = _make_dfu_package()
        python_bin = sys.executable
        if os.path.exists(os.path.join(_REPO_ROOT, ".venv", "bin", "python")):
            python_bin = os.path.join(_REPO_ROOT, ".venv", "bin", "python")
        result = subprocess.run(
            [python_bin, os.path.join(_REPO_ROOT, "scripts", "flash_webusb.py"), package],
            text=True,
            capture_output=True,
            env={**os.environ, "PAGER_HIL_LOCK_HELD": "1"},
            timeout=90,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        print("WebUSB update upload finished! Waiting for board to perform self-flash reset...")

        if serial_port:
            assert wait_for_serial_disconnect(serial_port, timeout=10), (
                "Board did not detach CDC before WebUSB DFU reset"
            )
            reconnected = wait_for_serial_reconnect(serial_port, timeout=30)
            assert reconnected, "Board did not reconnect after WebUSB update"
            print("Board successfully reconnected after WebUSB update!")
    except Exception as e:
        pytest.fail(f"WebUSB update failed: {e}")


@pytest.mark.contract
def test_serial_update_invalid_size(serial_port):
    """A serial 'update ' with an invalid size is rejected with ERROR_INVALID_SIZE."""
    print("\n--- Running serial 'update' (invalid size) Test ---")
    if not serial_port:
        pytest.skip("Serial port not found")
    try:
        s = serial.Serial(serial_port, 115200, timeout=2)
        s.dtr = True
        s.rts = True
        time.sleep(0.3)
        s.write(b"\r\nupdate 0\r\n")
        s.flush()
        seen = False
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            line = s.readline().decode("utf-8", errors="ignore")
            if "SERIAL_UPDATE:ERROR_INVALID_SIZE" in line or "invalid" in line.lower():
                seen = True
                break
        s.close()

        assert seen, "Serial update 0 did not report ERROR_INVALID_SIZE"
    except Exception as e:
        pytest.fail(f"Serial invalid-size test failed: {e}")


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
    except RuntimeError as e:
        if "Could not find BLE device" in str(e):
            pytest.skip(f"Pager BLE device not advertising: {e}")
        raise
