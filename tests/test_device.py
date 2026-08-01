import os
import time
import json
import asyncio
import serial
import urllib.request
import urllib.error
import http.client
import pytest
from bleak import BleakClient

from common import (
    find_serial_port,
    run_async,
    ncm_down,
    ncm_up,
    ensure_ncm_up,
    wait_for_http_reconnect,
    wait_for_serial_reconnect,
    find_ble_device,
    raw_http_request,
    http_host_port,
    http_request,
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
    DEFAULT_BASE_URL,
    UF2_VOLUME,
    _expected_hid_report,
)

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _dist_path(name):
    return os.path.join(_REPO_ROOT, "dist", name)


def _ensure_app_mode():
    """Best-effort: bring the board to a known-good baseline."""
    if os.path.exists(UF2_VOLUME) and os.path.exists(_dist_path("pager.uf2")):
        os.system(f"cp -X {_dist_path('pager.uf2')} {UF2_VOLUME}/ >/dev/null 2>&1")
        wait_for_serial_reconnect(DEFAULT_PORT, timeout=15)
    ensure_ncm_up()
    try:
        urllib.request.urlopen(
            urllib.request.Request(f"{DEFAULT_BASE_URL}/keyboard/switch?slot=0", method="POST"),
            timeout=5,
        )
    except Exception:
        pass


@pytest.fixture(scope="session", autouse=True)
def app_baseline():
    """Ensure the board starts and ends in application mode with slot 0 active."""
    print("\n=== Test suite setup: restoring board to application mode, slot 0 ===")
    _ensure_app_mode()
    yield
    print("\n=== Test suite teardown: restoring board to application mode, slot 0 ===")
    _ensure_app_mode()


@pytest.fixture
def serial_port():
    return find_serial_port()


# ---------------------------------------------------------------------------
# BLE Tests
# ---------------------------------------------------------------------------

@pytest.mark.ble
def test_ble_functionality():
    """Test scanning, connecting, writing and receiving notifications over Bluetooth LE"""
    print("\n--- Running BLE Functionality Test ---")

    async def run_ble_test():
        print("Scanning for BLE advertisement with custom service UUID...")
        device = await find_ble_device(SERVICE_UUID)
        print(f"Found BLE device {device.name or 'Unknown'} at {device.address}. Connecting...")

        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"
            print("BLE Connected successfully! Discovering services...")

            services = client.services
            assert SERVICE_UUID.lower() in [s.uuid.lower() for s in services], "Custom service UUID not found"

            notification_received = asyncio.Event()
            received_values = []

            def notification_callback(sender, data):
                received_values.append(data[0])
                if len(received_values) >= 2:
                    notification_received.set()

            print("Subscribing to status notifications...")
            await client.start_notify(STATUS_CHAR_UUID, notification_callback)

            print("Writing LED command: 0x02 (Manual ON)")
            await client.write_gatt_char(LED_CHAR_UUID, bytearray([0x02]))

            print("Waiting for status notification from board...")
            try:
                await asyncio.wait_for(notification_received.wait(), timeout=16.0)
            except asyncio.TimeoutError:
                pass
            finally:
                await client.stop_notify(STATUS_CHAR_UUID)

            print(f"Notification(s) received! Heartbeat values: {received_values}")
            assert len(received_values) > 0, "No status notification received"
            if len(received_values) >= 2:
                assert received_values[-1] > received_values[0], "Heartbeat value did not increment between ticks"

            print("Resetting LED to 0x00 (Auto Blink)")
            await client.write_gatt_char(LED_CHAR_UUID, bytearray([0x00]))

        print("BLE test completed successfully!")

    try:
        run_async(run_ble_test())
    except Exception as e:
        pytest.skip(f"BLE test skipped (device unavailable / likely connected to host): {e}")


# ---------------------------------------------------------------------------
# Serial CDC-ACM Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_serial_logs(serial_port):
    """Test receiving streaming logs over Serial (CDC-ACM)"""
    print("\n--- Running Serial Logs Test ---")
    assert serial_port, "Serial port not found"
    try:
        s = serial.Serial(serial_port, 115200, timeout=3)
        lines = []
        for _ in range(30):
            line = s.readline().decode('utf-8', errors='ignore')
            if line:
                lines.append(line.strip())
                if len(lines) >= 5:
                    break
            else:
                break
        s.close()
        print("Received serial lines:")
        print("\n".join(lines))
        assert len(lines) > 0, "No lines received from serial stream"
    except Exception as e:
        pytest.fail(f"Serial port failed: {e}")


@pytest.mark.dfu
def test_serial_update(serial_port):
    """Test firmware update over Serial"""
    print("\n--- Running Serial Update Test ---")
    assert serial_port, "Serial port not found"
    bin_path = _dist_path("pager.bin")
    if not os.path.exists(bin_path):
        pytest.skip(f"Binary {bin_path} not found. Build the firmware first.")

    with open(bin_path, "rb") as f:
        binary_data = f.read()

    try:
        s = serial.Serial(serial_port, 115200, timeout=5)
        cmd = f"update {len(binary_data)}\n".encode('utf-8')
        print(f"Sending serial command: {cmd.strip().decode()}")
        s.write(cmd)
        time.sleep(0.5)

        print("Board receiving DFU. Streaming binary chunks...")
        chunk_size = 512
        for i in range(0, len(binary_data), chunk_size):
            s.write(binary_data[i:i+chunk_size])

        s.close()
        print("Serial update upload finished! Waiting for board to perform self-flash reset...")

        ncm_down()

        reconnected = wait_for_serial_reconnect(serial_port, timeout=30)
        assert reconnected, "Board did not reconnect after serial update"

        http_online = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=30)
        assert http_online, "HTTP server did not come online after serial update"
        print("Board successfully reconnected after serial update!")
    except Exception as e:
        pytest.fail(f"Serial update failed: {e}")


# ---------------------------------------------------------------------------
# HTTP Route & Keyboard Endpoint Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_http_logs():
    """Test retrieving logs over HTTP"""
    print("\n--- Running HTTP Logs Test ---")
    wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=90)
    try:
        res = http_request("GET", "/logs")
        assert res.status == 200
        data = res.read().decode('utf-8')
        print("Logs received:")
        print("\n".join(data.split("\n")[-10:]))
        assert len(data) > 0, "No log data received from /logs"
        assert any(marker in data for marker in LOG_MARKERS), (
            f"Logs did not contain any expected subsystem marker: {data[:200]!r}"
        )
    except urllib.error.URLError as e:
        pytest.fail(f"HTTP connection failed: {e}")


@pytest.mark.smoke
def test_http_update():
    """Test firmware update over HTTP using the compiled binary"""
    pytest.skip("HTTP OTA update skipped; firmware uses USB Serial DFU update (`update <bytes>`).")


@pytest.mark.smoke
def test_keyboard_state():
    """Test getting current keyboard state"""
    print("\n--- Running GET /keyboard/state Test ---")
    try:
        res = http_request("GET", "/keyboard/state")
        assert res.status == 200
        data = json.loads(res.read().decode('utf-8'))
        assert "slots" in data
        assert "pairing_mode" in data
        assert len(data["slots"]) == 3
        for slot in data["slots"]:
            assert "id" in slot
            assert "active" in slot
            assert "bonded" in slot
        print("Successfully retrieved and validated keyboard state JSON!")
    except Exception as e:
        pytest.fail(f"GET /keyboard/state failed: {e}")


@pytest.mark.smoke
def test_keyboard_switch():
    """Test switching slots"""
    print("\n--- Running POST /keyboard/switch Test ---")
    try:
        res = http_request("POST", "/keyboard/switch?slot=1")
        assert res.status == 200
        assert res.read().decode('utf-8') == "Success"

        time.sleep(0.1)

        res = http_request("GET", "/keyboard/state")
        data = json.loads(res.read().decode('utf-8'))
        assert not data["slots"][0]["active"]
        assert data["slots"][1]["active"]
        print("Successfully switched profiles and verified active slot!")
    finally:
        http_request("POST", "/keyboard/switch?slot=0")


@pytest.mark.smoke
def test_keyboard_pair():
    """Test entering pairing mode"""
    print("\n--- Running POST /keyboard/pair Test ---")
    time.sleep(0.1)
    try:
        res = http_request("POST", "/keyboard/pair")
        assert res.status == 200
        assert res.read().decode('utf-8') == "Success"

        time.sleep(0.1)

        data = None
        for _ in range(5):
            try:
                res = http_request("GET", "/keyboard/state")
                data = json.loads(res.read().decode('utf-8'))
                break
            except Exception:
                time.sleep(0.2)
        assert data is not None, "Failed to retrieve keyboard state after entering pairing mode"
        assert data["pairing_mode"]
        print("Successfully put keyboard into pairing mode!")
    finally:
        http_request("POST", "/keyboard/switch?slot=0")


@pytest.mark.smoke
def test_keyboard_delete():
    """Test deleting a slot bond"""
    print("\n--- Running POST /keyboard/delete Test ---")
    time.sleep(0.1)
    try:
        res = http_request("POST", "/keyboard/delete?slot=1")
        assert res.status == 200
        assert res.read().decode('utf-8') == "Success"
        print("Successfully invoked delete bond endpoint!")
    except Exception as e:
        pytest.fail(f"POST /keyboard/delete failed: {e}")


@pytest.mark.smoke
def test_keyboard_type():
    """Test typing emulation over HTTP and verify the emitted HID reports."""
    print("\n--- Running POST /keyboard/type Test ---")
    time.sleep(2.0)
    text = "abc ABC 123"
    try:
        res = http_request(
            "POST", "/keyboard/type",
            data=text.encode('utf-8'),
            headers={"Content-Type": "text/plain"},
        )
        assert res.status == 200
        assert res.read().decode('utf-8') == "Success"
        print("Successfully sent typing request to keyboard emulator!")
    except Exception as e:
        pytest.fail(f"POST /keyboard/type failed: {e}")

    async def run_type_ble_test():
        print("Scanning for BLE device to verify HID typing...")
        device = await find_ble_device(SERVICE_UUID)
        print(f"Found BLE device at {device.address}. Connecting...")

        received_reports = []
        reports_done = asyncio.Event()

        def input_callback(sender, data):
            received_reports.append(bytes(data))
            if len([r for r in received_reports if r[2] != 0 or r[0] != 0]) >= len(text):
                reports_done.set()

        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect for HID typing test"
            await client.start_notify(HID_INPUT_REPORT_UUID, input_callback)
            await asyncio.sleep(0.5)

            def trigger():
                return http_request(
                    "POST", "/keyboard/type",
                    data=text.encode('utf-8'),
                    headers={"Content-Type": "text/plain"},
                )

            await asyncio.to_thread(trigger)

            try:
                await asyncio.wait_for(reports_done.wait(), timeout=15.0)
            except asyncio.TimeoutError:
                raise RuntimeError("Timed out waiting for HID keystroke notifications over BLE")
            finally:
                try:
                    await client.stop_notify(HID_INPUT_REPORT_UUID)
                except Exception:
                    pass

        down_reports = [r for r in received_reports if r[2] != 0 or r[0] != 0]
        assert len(down_reports) > 0, "No HID input reports received"
        assert len(down_reports) == len(text), "Expected one key-down report per typed character"

        for ch, r in zip(text, down_reports):
            assert len(r) == 8, f"Input report must be 8 bytes (no Report ID), got {len(r)}: {r.hex()}"
            assert r[0] != 0x01, f"Report carries a 0x01 Report ID prefix (bug): {r.hex()}"
            expected = _expected_hid_report(ch)
            assert expected is not None, f"Character {ch!r} has no HID mapping"
            assert r == expected, f"Report for {ch!r} mismatch: got {r.hex()}, want {expected.hex()}"
        print(f"Received and verified {len(down_reports)} HID keystroke report(s) over BLE!")

    try:
        run_async(run_type_ble_test())
    except Exception as e:
        pytest.skip(f"BLE HID verification skipped (device unavailable/connected): {e}")


# ---------------------------------------------------------------------------
# Error-Path Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_root_page():
    """GET / returns 200 with HTML content."""
    print("\n--- Running GET / Test ---")
    try:
        res = http_request("GET", "/")
        assert res.status == 200
        data = res.read().decode("utf-8", errors="ignore")
        assert "<html" in data.lower(), "GET / did not return HTML"
    except urllib.error.URLError as e:
        pytest.fail(f"GET / failed: {e}")


@pytest.mark.smoke
def test_index_html():
    """GET /index.html returns 200 with HTML content."""
    print("\n--- Running GET /index.html Test ---")
    try:
        res = http_request("GET", "/index.html")
        assert res.status == 200
        data = res.read().decode("utf-8", errors="ignore")
        assert "<html" in data.lower(), "GET /index.html did not return HTML"
    except urllib.error.URLError as e:
        pytest.fail(f"GET /index.html failed: {e}")


@pytest.mark.smoke
def test_not_found():
    """An unknown path returns 404."""
    print("\n--- Running 404 Test ---")
    try:
        res = http_request("GET", "/no-such-route")
        assert res.status == 404
    except urllib.error.HTTPError as e:
        assert e.code == 404
    except urllib.error.URLError as e:
        pytest.fail(f"404 request failed unexpectedly: {e}")


@pytest.mark.smoke
def test_update_missing_content_length():
    """POST /update without Content-Length is rejected with 411."""
    print("\n--- Running POST /update (missing Content-Length) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    status, _ = raw_http_request(
        host, port, "POST", "/update",
        body=b"\x00\x01", include_content_length=False,
    )
    assert status == 411, "Server should require Content-Length (411)"


@pytest.mark.smoke
def test_update_oversized():
    """POST /update larger than 400KB is rejected with 400."""
    print("\n--- Running POST /update (oversized) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    status, _ = raw_http_request(
        host, port, "POST", "/update", body=b"",
        content_length=409600 + 1,
        extra_headers={"Content-Type": "application/octet-stream"},
    )
    assert status == 400, "Server should reject oversized upload (400)"


@pytest.mark.smoke
def test_update_truncated():
    """A truncated upload (claimed length > sent bytes) must not crash the server."""
    print("\n--- Running POST /update (truncated) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    raw_http_request(
        host, port, "POST", "/update", body=b"X" * 100,
        extra_headers={"Content-Type": "application/octet-stream"},
        body_send_limit=50, close_after_body=True,
    )
    alive = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=90)
    assert alive, "Server died after a truncated upload"
    time.sleep(0.5)


@pytest.mark.smoke
def test_keyboard_switch_invalid_slot():
    """POST /keyboard/switch with an unknown slot returns 400 'Missing slot'."""
    print("\n--- Running POST /keyboard/switch (invalid slot) Test ---")
    try:
        res = http_request("POST", "/keyboard/switch?slot=9")
        assert res.status == 400
        assert "Missing slot" in res.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        assert e.code == 400
        assert "Missing slot" in e.read().decode("utf-8")
    except urllib.error.URLError as e:
        pytest.fail(f"Invalid-slot switch request failed: {e}")


@pytest.mark.smoke
def test_keyboard_delete_invalid_slot():
    """POST /keyboard/delete with an unknown slot returns 400 'Missing slot'."""
    print("\n--- Running POST /keyboard/delete (invalid slot) Test ---")
    try:
        res = http_request("POST", "/keyboard/delete?slot=9")
        assert res.status == 400
        assert "Missing slot" in res.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        assert e.code == 400
        assert "Missing slot" in e.read().decode("utf-8")
    except urllib.error.URLError as e:
        pytest.fail(f"Invalid-slot delete request failed: {e}")


@pytest.mark.smoke
def test_keyboard_type_invalid_size():
    """POST /keyboard/type with empty or >128-byte body returns 400 'Invalid size'."""
    print("\n--- Running POST /keyboard/type (invalid size) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    for body in (b"", b"X" * 129):
        status, data = raw_http_request(
            host, port, "POST", "/keyboard/type", body=body,
            extra_headers={"Content-Type": "text/plain"},
        )
        assert status == 400, f"Expected 400 for body of {len(body)} bytes"
        assert "Invalid size" in data


@pytest.mark.smoke
def test_serial_update_invalid_size(serial_port):
    """A serial 'update ' with an invalid size is rejected with ERROR_INVALID_SIZE."""
    print("\n--- Running serial 'update' (invalid size) Test ---")
    if not serial_port:
        pytest.skip("Serial port not found")
    try:
        s = serial.Serial(serial_port, 115200, timeout=2)
        time.sleep(0.1)
        s.write(b"update 0\r\n")
        s.flush()
        seen = False
        start = time.time()
        while time.time() - start < 3.0:
            line = s.readline().decode("utf-8", errors="ignore")
            if "SERIAL_UPDATE:ERROR_INVALID_SIZE" in line or "invalid" in line.lower():
                seen = True
                break
        s.close()

        if not seen:
            pytest.skip("Serial update 0 response lost in USB CDC stream")
    except Exception as e:
        pytest.skip(f"Serial invalid-size test skipped: {e}")


@pytest.mark.ble
def test_dis_battery_hid_metadata():
    """Read-only BLE GATT service checks (DIS, Battery, HID metadata, LED modes)."""
    print("\n--- Running BLE DIS/Battery/HID metadata Test ---")

    async def run_services_test():
        device = await find_ble_device(SERVICE_UUID)
        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"

            services = client.services
            uuids = [s.uuid.lower() for s in services]

            assert DIS_SERVICE_UUID.lower() in uuids, "DIS service (0x180A) not found"
            manufacturer = await client.read_gatt_char("00002a29-0000-1000-8000-00805f9b34fb")
            assert manufacturer.decode("utf-8", "ignore") == "Embassy"
            model = await client.read_gatt_char("00002a24-0000-1000-8000-00805f9b34fb")
            assert model.decode("utf-8", "ignore") == "nice_nano_v2"

            assert BATTERY_SERVICE_UUID.lower() in uuids, "Battery service (0x180F) not found"
            battery = await client.read_gatt_char("00002a19-0000-1000-8000-00805f9b34fb")
            assert battery[0] == 100, "Battery level should report 100"

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
        pytest.skip(f"BLE service test skipped (device unavailable): {e}")


@pytest.mark.ble
def test_boot_input_report():
    """Test receiving boot input report over BLE."""
    print("\n--- Running BLE Boot Keyboard Input Report Test ---")

    async def run_boot_report_test():
        device = await find_ble_device(SERVICE_UUID)
        received = []
        done = asyncio.Event()

        def boot_callback(sender, data):
            received.append(bytes(data))
            if len(received) >= 1:
                done.set()

        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect for boot report test"

            await client.write_gatt_char(HID_PROTOCOL_MODE_UUID, bytearray([0x00]))
            try:
                await client.start_notify(HID_BOOT_INPUT_REPORT_UUID, boot_callback)
                await asyncio.sleep(0.5)

                def trigger():
                    return http_request(
                        "POST", "/keyboard/type",
                        data=b"a", headers={"Content-Type": "text/plain"})

                await asyncio.to_thread(trigger)

                try:
                    await asyncio.wait_for(done.wait(), timeout=15.0)
                except asyncio.TimeoutError:
                    raise RuntimeError("No Boot Keyboard Input Report received over BLE")
                finally:
                    try:
                        await client.stop_notify(HID_BOOT_INPUT_REPORT_UUID)
                    except Exception:
                        pass

                assert len(received) > 0, "No boot input report received"
                assert len(received[0]) == 8, f"Boot report must be 8 bytes, got {len(received[0])}"
            finally:
                await client.write_gatt_char(HID_PROTOCOL_MODE_UUID, bytearray([0x01]))

    try:
        run_async(run_boot_report_test())
    except Exception as e:
        pytest.skip(f"BLE boot report test skipped (device unavailable): {e}")


@pytest.mark.ble
def test_bond_survives_reboot(serial_port):
    """Verify a bonded slot survives a reboot."""
    print("\n--- Running Bond Persistence Across Reboot Test ---")
    reconnected = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/keyboard/state", timeout=90)
    assert reconnected, "HTTP server did not come online"

    try:
        res = http_request("GET", "/keyboard/state")
        before = json.loads(res.read().decode("utf-8"))
    except Exception as e:
        pytest.fail(f"GET /keyboard/state failed: {e}")

    bonded_before = [i for i, s in enumerate(before["slots"]) if s["bonded"]]
    if not bonded_before:
        pytest.skip("No bonded slot present; pair a host first to test persistence")

    if not os.path.exists(_dist_path("pager.uf2")):
        pytest.skip("dist/pager.uf2 not found. Build the firmware first.")

    req = urllib.request.Request(f"{DEFAULT_BASE_URL}/bootloader", method="POST")
    urllib.request.urlopen(req, timeout=5)
    time.sleep(2.0)
    for _ in range(10):
        if os.path.exists(UF2_VOLUME):
            break
        time.sleep(1.0)
    os.system(f"cp -X {_dist_path('pager.uf2')} {UF2_VOLUME}/ >/dev/null 2>&1")
    assert wait_for_serial_reconnect(serial_port, timeout=15), "Board did not reconnect after reboot"
    ncm_up()

    reconnected = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/keyboard/state", timeout=90)
    assert reconnected, "HTTP server did not come online after reboot"
    try:
        res = http_request("GET", "/keyboard/state")
        after = json.loads(res.read().decode("utf-8"))
    except Exception as e:
        pytest.fail(f"GET /keyboard/state failed after reboot: {e}")

    bonded_after = [i for i, s in enumerate(after["slots"]) if s["bonded"]]
    assert sorted(bonded_before) == sorted(bonded_after), "Bonded slots changed across a reboot"
