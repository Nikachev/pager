import os
import time
import sys
import serial
import asyncio
import pytest
from bleak import BleakClient, BleakScanner

from common import (
    run_async,
    find_ble_device,
    HID_INPUT_REPORT_UUID,
    HID_PROTOCOL_MODE_UUID,
    HID_REPORT_MAP_UUID,
    BATTERY_SERVICE_UUID,
    BATTERY_LEVEL_UUID,
)

pytestmark = pytest.mark.hil

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


# ---------------------------------------------------------------------------
# BLE Functionality Tests
# ---------------------------------------------------------------------------


async def find_hil_ble_device(retries=3):
    for _attempt in range(retries):
        dev = await find_ble_device("Pager")
        if dev:
            return dev
        await asyncio.sleep(1.0)
    raise RuntimeError("Could not find BLE device 'Pager'")


def _decode_state(payload):
    from pager_tools.protocol import decode_state

    return decode_state(payload)


@pytest.mark.contract
def test_prepaired_slot_switching_fixture():
    """Switch 1 → 2 → 1 without pairing, mutation of slot 3, or USB loss."""
    import libusb_package
    from pager_tools.usb import PagerUsbClient, find_application

    backend = libusb_package.get_libusb1_backend()
    device = find_application(backend=backend, serial=os.getenv("PAGER_USB_SERIAL"))
    assert device is not None, "prepared Pager USB device not found"

    with PagerUsbClient(device) as client:
        initial = _decode_state(client.call(bytes([3])))
        assert initial["enabled"], "HIL fixture must start with Bluetooth enabled"
        assert initial["bonds"] == (True, True, False), (
            "HIL fixture must have slot 1=Mac, slot 2=other host, slot 3 empty"
        )

        # The second host is not controlled by HIL (typically an Android phone),
        # so activation must not require it to wake and initiate a connection.
        client.call(bytes([4, 1]), timeout_ms=12000)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            state = _decode_state(client.call(bytes([3])))
            if state["active"] == 1:
                break
            time.sleep(0.25)
        else:
            pytest.fail("slot 2 did not become active")
        assert state["bonds"] == (True, True, False)

        # The Mac running HIL is controlled and must reconnect with HID ready.
        client.call(bytes([4, 0]), timeout_ms=12000)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            state = _decode_state(client.call(bytes([3])))
            if state["connected"] == 0 and state["hid_ready"]:
                break
            time.sleep(0.25)
        else:
            pytest.fail("slot 1 did not reconnect with HID ready")
        assert state["bonds"] == (True, True, False)

        # Do not submit a chat message or form when the host's focused field
        # is used for manual HID acceptance.
        client.call(bytes([8]) + b"Pager HIL slot 1", timeout_ms=12000)


@pytest.mark.contract
def test_bluetooth_off_on_preserves_usb_and_bonds():
    """Stop the radio and reconnect slot 1 without releasing WebUSB."""
    import libusb_package
    from pager_tools.usb import PagerUsbClient, find_application

    backend = libusb_package.get_libusb1_backend()
    device = find_application(backend=backend, serial=os.getenv("PAGER_USB_SERIAL"))
    assert device is not None, "prepared Pager USB device not found"

    with PagerUsbClient(device) as client:
        client.call(bytes([6, 0]), timeout_ms=12000)
        off = _decode_state(client.call(bytes([3])))
        assert not off["enabled"]
        assert off["active"] is None
        assert off["connected"] is None
        assert off["bonds"] == (True, True, False)

        client.call(bytes([6, 1]), timeout_ms=12000)
        client.call(bytes([4, 0]), timeout_ms=12000)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            state = _decode_state(client.call(bytes([3])))
            if state["connected"] == 0 and state["hid_ready"]:
                break
            time.sleep(0.25)
        else:
            pytest.fail("slot 1 did not reconnect after Bluetooth Off/On")
        assert state["bonds"] == (True, True, False)


@pytest.mark.ble
def test_ble_functionality():
    """Connect to Pager and verify the standard HID keyboard profile."""
    if sys.platform == "darwin":
        pytest.skip(
            "macOS owns a paired HID connection; discovery/third-party GATT access "
            "is covered by manual pairing validation"
        )
    print("\n--- Running BLE Functionality Test ---")

    async def run_ble_test():
        device = await find_hil_ble_device()
        print(f"Found Pager BLE Device: {device.name} [{device.address}]...")

        if sys.platform == "darwin":
            advertisements = await BleakScanner.discover(timeout=3.0, return_adv=True)
            advertisement = next(
                (adv for found, adv in advertisements.values() if found.address == device.address),
                None,
            )
            assert advertisement is not None
            assert any(
                uuid.lower() == "1812" or uuid.lower().startswith("00001812-")
                for uuid in advertisement.service_uuids
            )

        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"
            print("Connected to BLE GATT server!")

            available_uuids = [c.uuid.lower() for s in client.services for c in s.characteristics]
            # macOS claims standard HID services for its system keyboard driver,
            # so CoreBluetooth intentionally exposes only the remaining GATT
            # services to Bleak. The HID UUID is verified in advertising above.
            if sys.platform != "darwin":
                assert HID_REPORT_MAP_UUID.lower() in available_uuids
                assert HID_INPUT_REPORT_UUID.lower() in available_uuids
            assert BATTERY_LEVEL_UUID.lower() in available_uuids

            print("BLE test completed successfully!")

    run_async(run_ble_test())


# ---------------------------------------------------------------------------
# CDC Serial Logs & DFU Reboot Tests
# ---------------------------------------------------------------------------


@pytest.mark.smoke
def test_serial_logs(serial_port):
    """Test retrieving live logs from CDC-ACM serial endpoint"""
    print("\n--- Running Serial Logs Test ---")
    port = serial_port
    try:
        s = serial.Serial(port, 115200, timeout=2)
        s.write(b"\r\n")
        s.flush()
        lines = []
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            line = s.readline().decode("utf-8", errors="ignore").strip()
            if line:
                lines.append(line)
                if len(lines) >= 3:
                    break
        s.close()
        full_text = "\n".join(lines)
        print("Serial logs received:")
        print(full_text)
        assert len(lines) > 0, "No logs received from CDC-ACM port"
    except (serial.SerialException, OSError) as e:
        pytest.fail(f"Serial port failed: {e}")


@pytest.mark.dfu
def test_uf2_flashing():
    """Test UF2 firmware flashing to Pager Bootloader"""
    print("\n--- Running UF2 Flashing Test ---")
    board = os.environ.get("PAGER_BOARD", "nice-nano-v2")
    uf2_file = os.path.join(_REPO_ROOT, "dist", board, "dev", "app", "pager.uf2")
    assert os.path.exists(uf2_file), f"UF2 file not found: {uf2_file}"

    sys.path.insert(0, os.path.join(_REPO_ROOT, "tools"))
    import flash_uf2 as flasher

    flasher.flash_uf2(uf2_file, board=board)
    print("UF2 Firmware transferred successfully and application booted!")


@pytest.mark.ble
def test_visible_gatt_metadata_and_hid_when_exposed():
    """Verify visible metadata and HID details when CoreBluetooth exposes them."""
    if sys.platform == "darwin":
        pytest.skip(
            "macOS hides the system-owned HID GATT database; metadata is covered "
            "by manual pairing validation"
        )
    print("\n--- Running BLE HID metadata/report Test ---")

    async def run_services_test():
        device = await find_hil_ble_device()
        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"

            services = client.services
            uuids = [s.uuid.lower() for s in services]

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

    run_async(run_services_test())


@pytest.mark.dfu
def test_corrupted_uf2_rejection():
    """Verify that UF2 blocks with corrupted magic or invalid parameters are rejected"""
    print("\n--- Running Corrupted UF2 Rejection Test ---")
    sys.path.insert(0, os.path.join(_REPO_ROOT, "tools"))
    import flash_uf2 as flasher

    bad_payload = bytearray(512)
    bad_payload[0:4] = (0xDEADBEEF).to_bytes(4, "little")  # Bad magic 0

    with pytest.raises(ValueError):
        flasher.flash_uf2_bytes(bytes(bad_payload))
    print("Corrupted UF2 payload correctly rejected by flasher / validation!")


@pytest.mark.smoke
def test_partition_limits():
    """Verify that built binary payloads fit within allocated Flash partitions"""
    board = os.environ.get("PAGER_BOARD", "nice-nano-v2")
    signed_bin = os.path.join(_REPO_ROOT, "dist", board, "dev", "app", "pager-signed.bin")
    assert os.path.exists(signed_bin), f"Build the selected board first: {signed_bin}"
    size = os.path.getsize(signed_bin)
    import json

    with open(os.path.join(_REPO_ROOT, "layout.json"), encoding="utf-8") as layout_file:
        layout = json.load(layout_file)
    limit = layout["storage_start"] - layout["firmware_start"]
    assert size <= limit, f"Signed payload {size} exceeds partition limit {limit}"
    print(f"Partition limit test passed: payload size {size} / {limit} bytes")
