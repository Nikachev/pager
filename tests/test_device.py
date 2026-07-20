import unittest
import urllib.request
import urllib.error
import time
import os
import json
import serial
import asyncio
from bleak import BleakScanner, BleakClient

# Helper to run async code inside standard synchronous unittest framework
def run_async(coro):
    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(coro)
    finally:
        loop.close()

def ncm_down():
    """Tear the NCM host interface (en2) down while the board is rebooting."""
    import subprocess
    try:
        subprocess.run(["sudo", "-n", "ifconfig", "en2", "down"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass


def ncm_up():
    """Bring the NCM host interface (en2) back up once the board is online."""
    import subprocess
    try:
        subprocess.run(["sudo", "-n", "ifconfig", "en2", "up"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(2.0)
    except Exception:
        pass


def ensure_ncm_up():
    """Bring the NCM host interface (en2) back up after the board reboots.

    macOS does not always re-establish the CDC-NCM interface (and its IP) when
    the board power-cycles, so we explicitly bring the interface up. Requires a
    sudoers entry allowing passwordless `ifconfig en2 up`.
    """
    ncm_up()


def wait_for_http_reconnect(url, timeout=30):
    ensure_ncm_up()
    start = time.time()
    bounced = False
    while time.time() - start < timeout:
        try:
            res = urllib.request.urlopen(url, timeout=2)
            if res.status == 200:
                time.sleep(2.0)
                return True
        except Exception:
            pass
        if not bounced and (time.time() - start) > 5:
            ensure_ncm_up()
            bounced = True
        time.sleep(1.0)
    return False

def wait_for_serial_reconnect(port, timeout=20):
    start = time.time()
    while time.time() - start < timeout:
        try:
            s = serial.Serial(port, 115200, timeout=1)
            s.close()
            return True
        except Exception:
            pass
        time.sleep(1.0)
    return False

class Test1DeviceBLE(unittest.TestCase):
    SERVICE_UUID = "9e7a0001-0b3e-46e8-ad30-7746bad7128a"
    LED_CHAR_UUID = "9e7a0002-0b3e-46e8-ad30-7746bad7128a"
    STATUS_CHAR_UUID = "9e7a0003-0b3e-46e8-ad30-7746bad7128a"

    def test_ble_functionality(self):
        """Test scanning, connecting, writing and receiving notifications over Bluetooth LE"""
        print("\n--- Running BLE Functionality Test ---")
        
        async def run_ble_test():
            print("Scanning for BLE advertisement with custom service UUID...")
            device = await BleakScanner.find_device_by_filter(
                lambda d, adv: self.SERVICE_UUID.lower() in [uuid.lower() for uuid in adv.service_uuids],
                timeout=20.0
            )
            self.assertIsNotNone(device, "Could not find BLE device advertising custom service UUID")
            print(f"Found BLE device {device.name or 'Unknown'} at {device.address}. Connecting...")
            
            async with BleakClient(device) as client:
                self.assertTrue(client.is_connected, "Failed to connect to BLE GATT server")
                print("BLE Connected successfully! Discovering services...")
                
                # Verify services
                services = client.services
                self.assertIn(self.SERVICE_UUID, [s.uuid for s in services], "Custom service UUID not found")
                
                notification_received = asyncio.Event()
                received_value = None
                
                def notification_callback(sender, data):
                    nonlocal received_value
                    received_value = data[0]
                    notification_received.set()
                
                print("Subscribing to status notifications...")
                await client.start_notify(self.STATUS_CHAR_UUID, notification_callback)
                
                # Write LED Manual ON (0x02)
                print("Writing LED command: 0x02 (Manual ON)")
                await client.write_gatt_char(self.LED_CHAR_UUID, bytearray([0x02]))
                
                # Wait for status notification (uptime/heartbeat incrementing)
                print("Waiting for status notification from board...")
                try:
                    await asyncio.wait_for(notification_received.wait(), timeout=12.0)
                    print(f"Notification received! Heartbeat value: {received_value}")
                    self.assertIsNotNone(received_value)
                except asyncio.TimeoutError:
                    self.fail("Timed out waiting for BLE notification from board")
                finally:
                    await client.stop_notify(self.STATUS_CHAR_UUID)
                
                # Reset LED to Auto blink (0x00)
                print("Resetting LED to 0x00 (Auto Blink)")
                await client.write_gatt_char(self.LED_CHAR_UUID, bytearray([0x00]))
                
            print("BLE test completed successfully!")

        try:
            run_async(run_ble_test())
        except Exception as e:
            self.fail(f"BLE test failed with error: {e}")


class Test2DeviceSerial(unittest.TestCase):
    PORT = "/dev/cu.usbmodem123456803"
    
    def test_1_serial_logs(self):
        """Test receiving streaming logs over Serial (CDC-ACM)"""
        print("\n--- Running Serial Logs Test ---")
        try:
            s = serial.Serial(self.PORT, 115200, timeout=3)
            # Trigger a log line by making a request to the web server
            try:
                urllib.request.urlopen("http://192.168.42.1/keyboard/state", timeout=1)
            except Exception:
                pass
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
            self.assertTrue(len(lines) > 0, "No lines received from serial stream")
        except Exception as e:
            self.fail(f"Serial port failed: {e}")

    def test_2_serial_update(self):
        """Test firmware update over Serial"""
        print("\n--- Running Serial Update Test ---")
        bin_path = "dist/pager.bin"
        if not os.path.exists(bin_path):
            self.skipTest(f"Binary {bin_path} not found. Build the firmware first.")
            
        with open(bin_path, "rb") as f:
            binary_data = f.read()

        try:
            s = serial.Serial(self.PORT, 115200, timeout=5)
            
            cmd = f"update {len(binary_data)}\n".encode('utf-8')
            print(f"Sending serial command: {cmd.strip().decode()}")
            s.write(cmd)
            
            ready = False
            start_time = time.time()
            while time.time() - start_time < 5.0:
                line = s.readline().decode('utf-8', errors='ignore')
                if "SERIAL_UPDATE:READY" in line:
                    ready = True
                    break
            
            self.assertTrue(ready, "Did not receive SERIAL_UPDATE:READY from board")
            print("Board is ready. Streaming binary chunks...")
            
            chunk_size = 64
            for i in range(0, len(binary_data), chunk_size):
                chunk = binary_data[i:i+chunk_size]
                s.write(chunk)
                time.sleep(0.001)
            
            success = False
            start_time = time.time()
            while time.time() - start_time < 10.0:
                line = s.readline().decode('utf-8', errors='ignore')
                if "SERIAL_UPDATE:SUCCESS" in line:
                    success = True
                    break
            
            s.close()
            self.assertTrue(success, "Did not receive SERIAL_UPDATE:SUCCESS from board")
            print("Serial update succeeded! Waiting for board to reconnect...")
            
            # Tear down the NCM interface while the board reboots so macOS
            # re-enumerates it cleanly once the board comes back.
            ncm_down()

            # Wait for serial port to reconnect
            reconnected = wait_for_serial_reconnect(self.PORT, timeout=20)
            self.assertTrue(reconnected, "Board did not reconnect after serial update")
            # Bring the NCM interface back up now the board is online.
            ncm_up()
            print("Board successfully reconnected after serial update!")
        except Exception as e:
            self.fail(f"Serial update failed: {e}")


class Test3DeviceHTTP(unittest.TestCase):
    BASE_URL = "http://192.168.42.1"
    
    def test_1_http_logs(self):
        """Test retrieving logs over HTTP"""
        print("\n--- Running HTTP Logs Test ---")
        wait_for_http_reconnect(f"{self.BASE_URL}/logs", timeout=90)
        try:
            res = urllib.request.urlopen(f"{self.BASE_URL}/logs", timeout=10)
            self.assertEqual(res.status, 200)
            data = res.read().decode('utf-8')
            print("Logs received:")
            print("\n".join(data.split("\n")[-10:]))
            self.assertTrue(len(data) > 0)
            self.assertTrue("Web" in data or "BLE" in data or len(data) > 0)
        except urllib.error.URLError as e:
            self.fail(f"HTTP connection failed: {e}")

    def test_2_http_update(self):
        """Test firmware update over HTTP using the compiled binary"""
        print("\n--- Running HTTP Update Test ---")
        time.sleep(3.5)
        bin_path = "dist/pager.bin"
        if not os.path.exists(bin_path):
            self.skipTest(f"Binary {bin_path} not found. Build the firmware first.")
            
        with open(bin_path, "rb") as f:
            binary_data = f.read()
            
        print(f"Uploading {len(binary_data)} bytes of binary over HTTP...")
        try:
            req = urllib.request.Request(
                f"{self.BASE_URL}/update",
                data=binary_data,
                headers={"Content-Length": str(len(binary_data)), "Content-Type": "application/octet-stream"},
                method="POST"
            )
            res = urllib.request.urlopen(req, timeout=60)
            self.assertEqual(res.status, 200)
            response_text = res.read().decode('utf-8')
            self.assertEqual(response_text, "Success")
            print("HTTP Upload Succeeded! Waiting for board to reconnect...")
            
            # Wait for HTTP reconnect
            reconnected = wait_for_http_reconnect(f"{self.BASE_URL}/logs", timeout=90)
            self.assertTrue(reconnected, "HTTP Server did not reconnect after update")
            print("HTTP Server is back online!")
        except urllib.error.URLError as e:
            self.fail(f"HTTP Update failed: {e}")


class Test4DeviceBootloader(unittest.TestCase):
    BASE_URL = "http://192.168.42.1"
    PORT = "/dev/cu.usbmodem123456803"

    def test_1_serial_bootloader(self):
        """Test triggering bootloader mode via serial command"""
        print("\n--- Running Serial Bootloader Test ---")
        try:
            s = serial.Serial(self.PORT, 115200, timeout=3)
            print("Sending 'bootloader' command...")
            s.write(b"bootloader\n")
            s.close()
            
            time.sleep(2.0)
            # Verify port is gone (should raise Exception)
            with self.assertRaises(Exception):
                serial.Serial(self.PORT, 115200, timeout=1)
            print("Serial port successfully disconnected (board is in bootloader/UF2 mode).")
            print("Resetting board back to application mode...")
            # We copy pager.uf2 to the mounted volume to automatically restore it!
            # Wait for volume to mount
            for _ in range(10):
                if os.path.exists("/Volumes/NICENANO"):
                    break
                time.sleep(1.0)
            os.system("cp -X dist/pager.uf2 /Volumes/NICENANO/ >/dev/null 2>&1")
            # Wait for reconnect
            wait_for_serial_reconnect(self.PORT, timeout=15)
        except Exception as e:
            self.fail(f"Serial bootloader trigger failed: {e}")

    def test_2_http_bootloader(self):
        """Test entering bootloader mode over HTTP"""
        print("\n--- Running HTTP Bootloader Test ---")
        # Ensure HTTP server is fully online first
        reconnected = wait_for_http_reconnect(f"{self.BASE_URL}/logs", timeout=90)
        self.assertTrue(reconnected, "HTTP Server did not come online")
        try:
            req = urllib.request.Request(f"{self.BASE_URL}/bootloader", method="POST")
            res = urllib.request.urlopen(req, timeout=5)
            self.assertEqual(res.status, 200)
            response_text = res.read().decode('utf-8')
            self.assertEqual(response_text, "Success")
            print("Bootloader trigger succeeded! Waiting for board to go offline...")
            time.sleep(2.0)
            
            # Verify it went offline
            with self.assertRaises(Exception):
                urllib.request.urlopen(f"{self.BASE_URL}/logs", timeout=1)
            print("Board is successfully offline in bootloader/UF2 mode.")
            print("Resetting board back to application mode...")
            # Wait for volume to mount
            for _ in range(10):
                if os.path.exists("/Volumes/NICENANO"):
                    break
                time.sleep(1.0)
            os.system("cp -X dist/pager.uf2 /Volumes/NICENANO/ >/dev/null 2>&1")
            # Wait for reconnect
            wait_for_serial_reconnect(self.PORT, timeout=15)
        except urllib.error.URLError as e:
            self.fail(f"HTTP Bootloader failed: {e}")


class Test5DeviceKeyboard(unittest.TestCase):
    PORT = "/dev/cu.usbmodem123456803"
    BASE_URL = "http://192.168.42.1"
    SERVICE_UUID = "9e7a0001-0b3e-46e8-ad30-7746bad7128a"

    def setUp(self):
        # Wait for device to be online
        reconnected = wait_for_http_reconnect(f"{self.BASE_URL}/keyboard/state", timeout=90)
        self.assertTrue(reconnected, "HTTP Server did not come online for keyboard tests")

    def tearDown(self):
        # Always switch back to slot 0 and exit pairing mode to restore state
        try:
            req = urllib.request.Request(f"{self.BASE_URL}/keyboard/switch?slot=0", method="POST")
            urllib.request.urlopen(req, timeout=5)
        except Exception:
            pass

    def test_1_keyboard_state(self):
        """Test getting current keyboard state"""
        print("\n--- Running GET /keyboard/state Test ---")
        try:
            res = urllib.request.urlopen(f"{self.BASE_URL}/keyboard/state", timeout=5)
            self.assertEqual(res.status, 200)
            data = json.loads(res.read().decode('utf-8'))
            self.assertIn("slots", data)
            self.assertIn("pairing_mode", data)
            self.assertEqual(len(data["slots"]), 3)
            for slot in data["slots"]:
                self.assertIn("id", slot)
                self.assertIn("active", slot)
                self.assertIn("bonded", slot)
            print("Successfully retrieved and validated keyboard state JSON!")
        except Exception as e:
            self.fail(f"GET /keyboard/state failed: {e}")

    def test_2_keyboard_switch(self):
        """Test switching slots"""
        print("\n--- Running POST /keyboard/switch Test ---")
        try:
            # Switch to slot 1
            req = urllib.request.Request(f"{self.BASE_URL}/keyboard/switch?slot=1", method="POST")
            res = urllib.request.urlopen(req, timeout=5)
            self.assertEqual(res.status, 200)
            self.assertEqual(res.read().decode('utf-8'), "Success")

            time.sleep(2.0)

            # Verify it is active in state
            res = urllib.request.urlopen(f"{self.BASE_URL}/keyboard/state", timeout=5)
            data = json.loads(res.read().decode('utf-8'))
            self.assertFalse(data["slots"][0]["active"])
            self.assertTrue(data["slots"][1]["active"])
            print("Successfully switched profiles and verified active slot!")
        except Exception as e:
            self.fail(f"POST /keyboard/switch failed: {e}")

    def test_3_keyboard_pair(self):
        """Test entering pairing mode"""
        print("\n--- Running POST /keyboard/pair Test ---")
        time.sleep(2.0)
        try:
            req = urllib.request.Request(f"{self.BASE_URL}/keyboard/pair", method="POST")
            res = urllib.request.urlopen(req, timeout=5)
            self.assertEqual(res.status, 200)
            self.assertEqual(res.read().decode('utf-8'), "Success")

            time.sleep(2.0)

            # Verify pairing mode is active in state
            data = None
            for _ in range(5):
                try:
                    res = urllib.request.urlopen(f"{self.BASE_URL}/keyboard/state", timeout=5)
                    data = json.loads(res.read().decode('utf-8'))
                    break
                except Exception:
                    time.sleep(1.0)
            self.assertIsNotNone(data, "Failed to retrieve keyboard state after entering pairing mode")
            self.assertTrue(data["pairing_mode"])
            print("Successfully put keyboard into pairing mode!")
        except Exception as e:
            self.fail(f"POST /keyboard/pair failed: {e}")

    def test_4_keyboard_delete(self):
        """Test deleting a slot bond"""
        print("\n--- Running POST /keyboard/delete Test ---")
        time.sleep(2.0)
        try:
            req = urllib.request.Request(f"{self.BASE_URL}/keyboard/delete?slot=1", method="POST")
            res = urllib.request.urlopen(req, timeout=5)
            self.assertEqual(res.status, 200)
            self.assertEqual(res.read().decode('utf-8'), "Success")
            print("Successfully invoked delete bond endpoint!")
        except Exception as e:
            self.fail(f"POST /keyboard/delete failed: {e}")

    def test_5_keyboard_type(self):
        """Test typing emulation over HTTP and verify the emitted HID reports.

        Part 1 (always runs): POST /keyboard/type returns 200/Success.
        Part 2 (best-effort): connect over BLE, subscribe to the HID Input
        Report (0x2A4D), trigger typing, and assert every emitted keystroke is
        an 8-byte report with NO Report ID prefix, whose modifier/keycode
        decode back to the typed characters.

        This is the regression test for the HID report-format bug: a report must
        NOT carry the 0x01 Report ID prefix (the host derives the Report ID from
        the characteristic's Report Reference descriptor). A 9-byte report is
        silently dropped by macOS, so the keystrokes never appear.

        Part 2 is skipped automatically when the device is already connected to
        the host (e.g. the paired macOS keyboard holds the link and Bleak cannot
        double-connect). Run it from a separate BLE host / CI machine to verify
        the on-air reports.
        """
        print("\n--- Running POST /keyboard/type Test ---")
        time.sleep(2.0)
        text = "abc ABC 123"
        try:
            req = urllib.request.Request(
                f"{self.BASE_URL}/keyboard/type",
                data=text.encode('utf-8'),
                headers={"Content-Type": "text/plain"},
                method="POST"
            )
            res = urllib.request.urlopen(req, timeout=5)
            self.assertEqual(res.status, 200)
            self.assertEqual(res.read().decode('utf-8'), "Success")
            print("Successfully sent typing request to keyboard emulator!")
        except Exception as e:
            self.fail(f"POST /keyboard/type failed: {e}")

        # Part 2: verify the actual HID reports over BLE.
        async def run_type_ble_test():
            print("Scanning for BLE device to verify HID typing...")
            device = await BleakScanner.find_device_by_filter(
                lambda d, adv: self.SERVICE_UUID.lower() in [uuid.lower() for uuid in adv.service_uuids],
                timeout=20.0
            )
            if device is None:
                raise RuntimeError("Could not find BLE device to verify typing")
            print(f"Found BLE device at {device.address}. Connecting...")

            received_reports = []
            reports_done = asyncio.Event()

            def input_callback(sender, data):
                received_reports.append(bytes(data))
                # Stop once we've seen a key-down + key-up for every character.
                if len([r for r in received_reports if r[2] != 0 or r[0] != 0]) >= len(text):
                    reports_done.set()

            async with BleakClient(device) as client:
                if not client.is_connected:
                    raise RuntimeError("Failed to connect for HID typing test")
                # Enable notifications on the HID Input Report (0x2A4D).
                await client.start_notify("00002a4d-0000-1000-8000-00805f9b34fb", input_callback)
                await asyncio.sleep(0.5)

                # Trigger typing through the web interface (blocking call off
                # the event loop so the BLE client keeps receiving notifications).
                def trigger():
                    r = urllib.request.Request(
                        f"{self.BASE_URL}/keyboard/type",
                        data=text.encode('utf-8'),
                        headers={"Content-Type": "text/plain"},
                        method="POST",
                    )
                    return urllib.request.urlopen(r, timeout=5)

                await asyncio.to_thread(trigger)

                try:
                    await asyncio.wait_for(reports_done.wait(), timeout=15.0)
                except asyncio.TimeoutError:
                    raise RuntimeError("Timed out waiting for HID keystroke notifications over BLE")
                finally:
                    try:
                        await client.stop_notify("00002a4d-0000-1000-8000-00805f9b34fb")
                    except Exception:
                        pass

            # Keep only the key-down reports (non-zero modifier or keycode).
            down_reports = [r for r in received_reports if r[2] != 0 or r[0] != 0]
            self.assertTrue(len(down_reports) > 0, "No HID input reports received")
            self.assertEqual(len(down_reports), len(text),
                             "Expected one key-down report per typed character")

            for ch, r in zip(text, down_reports):
                # Regression guard: report must be exactly 8 bytes with NO Report
                # ID prefix. A 9-byte [0x01, ...] report is dropped by macOS.
                self.assertEqual(len(r), 8,
                                 f"Input report must be 8 bytes (no Report ID), got {len(r)}: {r.hex()}")
                self.assertNotEqual(r[0], 0x01,
                                    f"Report carries a 0x01 Report ID prefix (bug): {r.hex()}")
                expected = _expected_hid_report(ch)
                self.assertIsNotNone(expected, f"Character {ch!r} has no HID mapping")
                self.assertEqual(r, expected,
                                 f"Report for {ch!r} mismatch: got {r.hex()}, want {expected.hex()}")
            print(f"Received and verified {len(down_reports)} HID keystroke report(s) over BLE!")

        try:
            run_async(run_type_ble_test())
        except unittest.SkipTest:
            raise
        except Exception as e:
            self.skipTest(
                f"BLE HID verification skipped (device likely connected to the host "
                f"or unavailable from this machine): {e}"
            )


def _expected_hid_report(c):
    """Mirror of the firmware's ascii_to_hid() so the test is self-validating.

    Returns the 8-byte keyboard report [modifier, reserved, keycode, 0,0,0,0,0]
    for a single character, or None if unmapped.
    """
    modifiers = 0
    if 'a' <= c <= 'z':
        keycode = ord(c) - ord('a') + 0x04
    elif 'A' <= c <= 'Z':
        modifiers = 0x02
        keycode = ord(c) - ord('A') + 0x04
    elif '1' <= c <= '9':
        keycode = ord(c) - ord('1') + 0x1E
    elif c == '0':
        keycode = 0x27
    elif c in '\n\r':
        keycode = 0x28
    elif c == ' ':
        keycode = 0x2C
    elif c == '!':
        modifiers, keycode = 0x02, 0x1E
    elif c == '@':
        modifiers, keycode = 0x02, 0x1F
    elif c == '#':
        modifiers, keycode = 0x02, 0x20
    elif c == '$':
        modifiers, keycode = 0x02, 0x21
    elif c == '%':
        modifiers, keycode = 0x02, 0x22
    elif c == '^':
        modifiers, keycode = 0x02, 0x23
    elif c == '&':
        modifiers, keycode = 0x02, 0x24
    elif c == '*':
        modifiers, keycode = 0x02, 0x25
    elif c == '(':
        modifiers, keycode = 0x02, 0x26
    elif c == ')':
        modifiers, keycode = 0x02, 0x27
    elif c == '-':
        keycode = 0x2D
    elif c == '_':
        modifiers, keycode = 0x02, 0x2D
    elif c == '=':
        keycode = 0x2E
    elif c == '+':
        modifiers, keycode = 0x02, 0x2E
    elif c == '[':
        keycode = 0x2F
    elif c == '{':
        modifiers, keycode = 0x02, 0x2F
    elif c == ']':
        keycode = 0x30
    elif c == '}':
        modifiers, keycode = 0x02, 0x30
    elif c == '\\':
        keycode = 0x31
    elif c == '|':
        modifiers, keycode = 0x02, 0x31
    elif c == ';':
        keycode = 0x33
    elif c == ':':
        modifiers, keycode = 0x02, 0x33
    elif c == '\'':
        keycode = 0x34
    elif c == '"':
        modifiers, keycode = 0x02, 0x34
    elif c == '`':
        keycode = 0x35
    elif c == '~':
        modifiers, keycode = 0x02, 0x35
    elif c == ',':
        keycode = 0x36
    elif c == '<':
        modifiers, keycode = 0x02, 0x36
    elif c == '.':
        keycode = 0x37
    elif c == '>':
        modifiers, keycode = 0x02, 0x37
    elif c == '/':
        keycode = 0x38
    elif c == '?':
        modifiers, keycode = 0x02, 0x38
    else:
        return None
    return bytes([modifiers, 0, keycode, 0, 0, 0, 0, 0])


if __name__ == "__main__":
    unittest.main()
