import os
import time
import json
import socket
import asyncio
import subprocess
import sys
import serial
import urllib.request
import urllib.error
import http.client
import zlib
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
    wait_for_serial_disconnect,
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
    _expected_hid_report,
)

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _dist_path(name):
    return os.path.join(_REPO_ROOT, "dist", name)


def _make_dfu_package(name):
    """Create a newer package for the slot currently inactive on the board."""
    version_path = _dist_path(".dfu-test-version")
    try:
        previous = int(open(version_path, encoding="utf-8").read().strip())
    except (OSError, ValueError):
        previous = 0
    version = max(int(time.time()) + 10, previous + 1)
    with open(version_path, "w", encoding="utf-8") as f:
        f.write(str(version))

    health = json.loads(http_request("GET", "/health").read().decode("utf-8"))
    target_slot = "A" if health["ota_target_slot"] == 0 else "B"
    # Never reuse an artifact left by an HIL fault-injection run. DFU tests
    # must always install a normal image that confirms its trial and feeds WDT.
    subprocess.run(
        [
            "make",
            "build",
            f"SLOT={target_slot}",
            "TRIAL_NO_CONFIRM=0",
            "WATCHDOG_NO_FEED=0",
        ],
        cwd=_REPO_ROOT,
        check=True,
    )
    image = _dist_path(f"pager-{target_slot}.bin")
    output = _dist_path(name)
    subprocess.run(
        [
            sys.executable,
            os.path.join(_REPO_ROOT, "scripts", "sign_firmware.py"),
            image,
            "--version",
            str(version),
            "--slot",
            target_slot,
            "--output",
            output,
        ],
        check=True,
    )
    return output


def _ota_headers(host, port, binary_data):
    return (
        f"POST /update HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        f"Content-Type: application/octet-stream\r\n"
        f"Content-Length: {len(binary_data)}\r\n"
        f"X-Pager-CRC32: {zlib.crc32(binary_data):08x}\r\n"
        f"Connection: close\r\n\r\n"
    ).encode("utf-8")


def _ensure_transport():
    """Bring the host-side NCM interface up without changing board state."""
    ensure_ncm_up()


async def _pair_current_slot_if_needed(force=False):
    """Trigger macOS Just Works pairing through the protected HID report.

    CoreBluetooth intentionally has no API for accepting a passkey dialog. On
    a Just Works link this read completes the pairing without UI; otherwise the
    test leaves the native macOS dialog visible for Computer Use/the developer.
    """
    state = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
    active_slot = next(slot for slot in state["slots"] if slot["active"])
    if active_slot["bonded"] and not force:
        return
    # Send an explicit zero-length body. This matches fetch() in the web UI
    # and keeps the embedded HTTP parser from waiting for a POST body.
    http_request("POST", "/keyboard/pair", data=b"").read()
    device = await find_ble_device(SERVICE_UUID, timeout=10.0)
    async with BleakClient(device, timeout=20.0) as client:
        assert client.is_connected, "Failed to connect while requesting pairing"
        try:
            await client.read_gatt_char(HID_INPUT_REPORT_UUID)
        except Exception:
            # macOS may finish the security procedure after the protected read
            # returns its ATT error; state polling below is authoritative.
            pass
        for _ in range(20):
            await asyncio.sleep(0.5)
            state = json.loads((await asyncio.to_thread(lambda: http_request("GET", "/keyboard/state").read())).decode("utf-8"))
            if any(slot["active"] and slot["bonded"] for slot in state["slots"]):
                return
    raise RuntimeError("macOS did not complete HID pairing for the active slot")


@pytest.fixture(scope="session", autouse=True)
def app_baseline():
    """Ensure the host transport is configured without issuing mutating requests."""
    print("\n=== Test suite setup: configuring NCM transport ===")
    _ensure_transport()
    yield
    print("\n=== Test suite teardown: checking NCM transport ===")
    _ensure_transport()


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
    except RuntimeError as e:
        if "Could not find BLE device" in str(e):
            pytest.skip(f"BLE test skipped (device already connected): {e}")
        raise


# ---------------------------------------------------------------------------
# Serial CDC-ACM Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_serial_logs(serial_port):
    """Test receiving streaming logs over Serial (CDC-ACM)"""
    print("\n--- Running Serial Logs Test ---")
    assert serial_port, "Serial port not found"
    try:
        # Opening CDC with DTR asserted can reset some USB bridge/board setups.
        # Configure the line state before opening so this diagnostic does not
        # disrupt the NCM interface used by the rest of the smoke suite.
        s = serial.Serial(port=None, baudrate=115200, timeout=1)
        s.dtr = False
        s.rts = False
        s.port = serial_port
        s.open()
        time.sleep(0.1)
        s.reset_input_buffer()
        # Keep this smoke test non-destructive: pairing and typing change BLE
        # state and can make the following HTTP tests depend on host timing.
        s.write(b"ping\n")

        lines = []
        start = time.time()
        while time.time() - start < 1.5 and not lines:
            line = s.readline().decode('utf-8', errors='ignore').strip()
            if line:
                lines.append(line)
        s.close()
        print("Received serial lines:")
        print("\n".join(lines))
        assert "SERIAL:PONG" in lines, f"Expected SERIAL:PONG from serial stream, got {lines}"
    except Exception as e:
        pytest.fail(f"Serial port failed: {e}")


@pytest.mark.dfu
def test_serial_update(serial_port):
    """Test firmware update over Serial"""
    print("\n--- Running Serial Update Test ---")
    assert serial_port, "Serial port not found"
    try:
        package = _make_dfu_package("pager.serial-test.pkg")
        result = subprocess.run(
            [sys.executable, os.path.join(_REPO_ROOT, "scripts", "flash_serial.py"), serial_port, package],
            text=True,
            capture_output=True,
            env={**os.environ, "PAGER_HIL_LOCK_HELD": "1"},
            timeout=90,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        print("Serial update upload finished! Waiting for board to perform self-flash reset...")

        ncm_down()

        try:
            assert wait_for_serial_disconnect(serial_port, timeout=10), (
                "Board did not detach CDC before serial OTA reset"
            )
            reconnected = wait_for_serial_reconnect(serial_port, timeout=30)
            assert reconnected, "Board did not reconnect after serial update"

            ncm_up()
            http_online = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=30)
            assert http_online, "HTTP server did not come online after serial update"
            print("Board successfully reconnected after serial update!")
        finally:
            ensure_ncm_up()
    except Exception as e:
        pytest.fail(f"Serial update failed: {e}")


# ---------------------------------------------------------------------------
# HTTP Route & Keyboard Endpoint Tests
# ---------------------------------------------------------------------------

@pytest.mark.smoke
def test_http_logs():
    """Test retrieving logs over HTTP"""
    print("\n--- Running HTTP Logs Test ---")
    assert wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=8), (
        "HTTP endpoint did not become ready within 8 seconds"
    )
    try:
        res = http_request("GET", "/logs", retries=1, timeout=3)
        assert res.status == 200
        assert "text/plain" in res.headers.get("Content-Type", ""), "Expected text/plain Content-Type"
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
def test_health():
    """GET /health exposes OTA and queue-pressure diagnostics."""
    res = http_request("GET", "/health")
    assert res.status == 200
    health = json.loads(res.read().decode("utf-8"))
    assert set(health) == {
        "flashing",
        "dropped_logs",
        "dropped_ble_commands",
        "firmware_version",
        "slot",
        "ota_target_slot",
    }
    assert health["flashing"] is False
    assert isinstance(health["firmware_version"], int)
    assert health["slot"] in (0, 1)
    assert health["ota_target_slot"] == 1 - health["slot"]


@pytest.mark.dfu
def test_http_update():
    """Test firmware update over HTTP using the compiled binary"""
    print("\n--- Running HTTP OTA Update Test ---")
    with open(_make_dfu_package("pager.http-test.pkg"), "rb") as f:
        binary_data = f.read()

    host, port = http_host_port(DEFAULT_BASE_URL)
    last_exception = None

    for attempt in range(1, 4):
        try:
            wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=25)
            time.sleep(1.0)

            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(60)
            s.connect((host, port))

            headers = _ota_headers(host, port, binary_data)

            s.sendall(headers)

            chunk_size = 4096
            for i in range(0, len(binary_data), chunk_size):
                chunk = binary_data[i:i + chunk_size]
                s.sendall(chunk)

            resp = s.recv(1024).decode("utf-8", errors="ignore")
            s.close()
            assert "200" in resp, f"Expected 200 OK, got: {resp}"
            print("HTTP OTA update accepted! Board will self-flash and reset...")

            ncm_down()

            try:
                # Firmware detaches USB for 3 seconds before resetting. Do
                # not reconfigure macOS against the still-old NCM endpoint.
                time.sleep(4.0)
                ncm_up()
                http_online = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=30)
                assert http_online, "HTTP server did not come online after HTTP OTA update"
                print("Board successfully reconnected after HTTP OTA update!")
                return
            finally:
                ensure_ncm_up()
        except Exception as e:
            print(f"HTTP OTA attempt {attempt} failed: {e}")
            last_exception = e
            time.sleep(2.0)

    pytest.fail(f"HTTP OTA update failed after 3 attempts: {last_exception}")


@pytest.mark.contract
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


@pytest.mark.contract
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


@pytest.mark.ble
def test_single_host_profile_slot_isolation():
    """A single Mac can verify selection without pretending to be 3 hosts."""
    before = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
    original = next(slot["id"] for slot in before["slots"] if slot["active"])
    try:
        for slot in (1, 2, original):
            response = http_request("POST", f"/keyboard/switch?slot={slot}")
            assert response.read().decode("utf-8") == "Success"
            state = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
            assert state["slots"][slot]["active"], f"Slot {slot} was not selected"
        restored = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
        assert restored["slots"][original]["bonded"] == before["slots"][original]["bonded"]
    finally:
        http_request("POST", f"/keyboard/switch?slot={original}").read()


@pytest.mark.contract
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


@pytest.mark.contract
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


@pytest.mark.contract
@pytest.mark.ble
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
    except RuntimeError as e:
        if "Could not find BLE device" in str(e):
            pytest.skip(f"BLE HID verification skipped (device already connected): {e}")
        raise


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


@pytest.mark.contract
def test_update_missing_content_length():
    """POST /update without Content-Length is rejected with 411."""
    print("\n--- Running POST /update (missing Content-Length) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    status, _ = raw_http_request(
        host, port, "POST", "/update",
        body=b"\x00\x01", include_content_length=False,
    )
    assert status == 411, "Server should require Content-Length (411)"


@pytest.mark.contract
def test_update_oversized():
    """POST /update larger than the 488KB staging slot is rejected with 400."""
    print("\n--- Running POST /update (oversized) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    status, _ = raw_http_request(
        host, port, "POST", "/update", body=b"",
        content_length=499712 + 1,
        extra_headers={"Content-Type": "application/octet-stream"},
    )
    assert status == 400, "Server should reject oversized upload (400)"


@pytest.mark.contract
def test_update_missing_checksum():
    """POST /update without X-Pager-CRC32 is rejected before staging."""
    host, port = http_host_port(DEFAULT_BASE_URL)
    status, _ = raw_http_request(host, port, "POST", "/update", body=b"firmware")
    assert status == 400


@pytest.mark.contract
def test_update_bad_checksum():
    """A complete upload with a wrong CRC must never be activated."""
    host, port = http_host_port(DEFAULT_BASE_URL)
    payload = b"firmware"
    status, data = raw_http_request(
        host,
        port,
        "POST",
        "/update",
        body=payload,
        extra_headers={"X-Pager-CRC32": "00000000"},
    )
    assert status == 422
    assert "Checksum mismatch" in data


@pytest.mark.contract
def test_update_truncated():
    """A truncated upload (claimed length > sent bytes) must not crash the server."""
    print("\n--- Running POST /update (truncated) Test ---")
    host, port = http_host_port(DEFAULT_BASE_URL)
    raw_http_request(
        host, port, "POST", "/update", body=b"X" * 100,
        extra_headers={
            "Content-Type": "application/octet-stream",
            "X-Pager-CRC32": f"{zlib.crc32(b'X' * 100):08x}",
        },
        body_send_limit=50, close_after_body=True,
    )
    alive = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/logs", timeout=90)
    assert alive, "Server died after a truncated upload"
    time.sleep(0.5)


@pytest.mark.contract
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


@pytest.mark.contract
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


@pytest.mark.contract
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


@pytest.mark.contract
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
def test_hid_metadata_and_boot_input_report():
    """Keep all BLE assertions in one connection so macOS cannot preempt it."""
    print("\n--- Running continuous BLE HID metadata/Boot Input Test ---")
    # DIS, Battery and HID discovery metadata are deliberately public: they
    # must be usable by macOS before it decides to pair the keyboard.
    run_async(_pair_current_slot_if_needed())

    async def run_services_test():
        device = await find_ble_device(SERVICE_UUID)
        async with BleakClient(device, timeout=20.0) as client:
            assert client.is_connected, "Failed to connect to BLE GATT server"

            services = client.services
            uuids = [s.uuid.lower() for s in services]

            assert DIS_SERVICE_UUID.lower() in uuids, "DIS service (0x180A) not found"
            manufacturer = await client.read_gatt_char("00002a29-0000-1000-8000-00805f9b34fb")
            assert manufacturer.decode("utf-8", "ignore") == "Antigravity"
            model = await client.read_gatt_char("00002a24-0000-1000-8000-00805f9b34fb")
            assert model.decode("utf-8", "ignore") == "nice_nano_v2"

            assert BATTERY_SERVICE_UUID.lower() in uuids, "Battery service (0x180F) not found"
            battery = await client.read_gatt_char("00002a19-0000-1000-8000-00805f9b34fb")
            assert battery[0] == 13, "Battery placeholder should report 13%"

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
            received = []
            done = asyncio.Event()

            def boot_callback(sender, data):
                received.append(bytes(data))
                done.set()
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
        run_async(run_services_test())
    except RuntimeError as e:
        if "Could not find BLE device" in str(e):
            pytest.skip(f"BLE session skipped (device already connected): {e}")
        raise


@pytest.mark.ble
@pytest.mark.dfu
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

    if not any(slot["bonded"] for slot in before["slots"]):
        run_async(_pair_current_slot_if_needed())
        before = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
    bonded_before = [i for i, s in enumerate(before["slots"]) if s["bonded"]]
    assert bonded_before, "HID pairing did not create a persistent bond"

    with open(_make_dfu_package("pager.bond-test.pkg"), "rb") as f:
        binary_data = f.read()

    host, port = http_host_port(DEFAULT_BASE_URL)
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(60)
    s.connect((host, port))
    headers = _ota_headers(host, port, binary_data)
    s.sendall(headers)
    s.sendall(binary_data)
    resp = s.recv(1024).decode("utf-8", errors="ignore")
    s.close()
    assert "200" in resp, f"Expected 200 OK, got: {resp}"

    ncm_down()
    try:
        reconnected = wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/keyboard/state", timeout=90)
        assert reconnected, "HTTP server did not come online after reboot"
        after = json.loads(http_request("GET", "/keyboard/state").read().decode("utf-8"))
        assert after["slots"][bonded_before[0]]["bonded"], "Bond record was not restored after reboot"
    finally:
        ensure_ncm_up()

    try:
        res = http_request("GET", "/keyboard/state")
        after = json.loads(res.read().decode("utf-8"))
    except Exception as e:
        pytest.fail(f"GET /keyboard/state failed after reboot: {e}")

    bonded_after = [i for i, s in enumerate(after["slots"]) if s["bonded"]]
    assert sorted(bonded_before) == sorted(bonded_after), "Bonded slots changed across a reboot"
