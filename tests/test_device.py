import unittest
import urllib.request
import urllib.error
import time
import os
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

def wait_for_http_reconnect(url, timeout=20):
    start = time.time()
    while time.time() - start < timeout:
        try:
            res = urllib.request.urlopen(url, timeout=2)
            if res.status == 200:
                return True
        except Exception:
            pass
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
            print("Scanning for 'nice_nano' Bluetooth advertisement...")
            device = await BleakScanner.find_device_by_name("nice_nano", timeout=10.0)
            self.assertIsNotNone(device, "Could not find BLE device named 'nice_nano'")
            print(f"Found BLE device at {device.address}. Connecting...")
            
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
            # Read multiple lines to find heartbeat
            found_heartbeat = False
            lines = []
            for _ in range(30):
                line = s.readline().decode('utf-8', errors='ignore')
                if line:
                    lines.append(line.strip())
                if "Heartbeat" in line:
                    found_heartbeat = True
                    break
            s.close()
            print("Received serial lines:")
            print("\n".join(lines[-5:]))
            self.assertTrue(found_heartbeat, "Did not find Heartbeat in serial stream")
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
            
            # Wait for serial port to reconnect
            reconnected = wait_for_serial_reconnect(self.PORT, timeout=20)
            self.assertTrue(reconnected, "Board did not reconnect after serial update")
            print("Board successfully reconnected after serial update!")
        except Exception as e:
            self.fail(f"Serial update failed: {e}")


class Test3DeviceHTTP(unittest.TestCase):
    BASE_URL = "http://192.168.42.1"
    
    def test_1_http_logs(self):
        """Test retrieving logs over HTTP"""
        print("\n--- Running HTTP Logs Test ---")
        try:
            res = urllib.request.urlopen(f"{self.BASE_URL}/logs", timeout=5)
            self.assertEqual(res.status, 200)
            data = res.read().decode('utf-8')
            print("Logs received:")
            print("\n".join(data.split("\n")[-10:]))
            self.assertTrue(len(data) > 0)
            self.assertIn("Heartbeat", data)
        except urllib.error.URLError as e:
            self.fail(f"HTTP connection failed: {e}")

    def test_2_http_update(self):
        """Test firmware update over HTTP using the compiled binary"""
        print("\n--- Running HTTP Update Test ---")
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
            res = urllib.request.urlopen(req, timeout=15)
            self.assertEqual(res.status, 200)
            response_text = res.read().decode('utf-8')
            self.assertEqual(response_text, "Success")
            print("HTTP Upload Succeeded! Waiting for board to reconnect...")
            
            # Wait for HTTP reconnect
            reconnected = wait_for_http_reconnect(f"{self.BASE_URL}/logs", timeout=20)
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
            # Wait a few seconds for volume to mount
            time.sleep(2.0)
            os.system("cp dist/pager.uf2 /Volumes/NICENANO/ >/dev/null 2>&1")
            # Wait for reconnect
            wait_for_serial_reconnect(self.PORT, timeout=15)
        except Exception as e:
            self.fail(f"Serial bootloader trigger failed: {e}")

    def test_2_http_bootloader(self):
        """Test entering bootloader mode over HTTP"""
        print("\n--- Running HTTP Bootloader Test ---")
        # Ensure HTTP server is fully online first
        wait_for_http_reconnect(f"{self.BASE_URL}/logs", timeout=15)
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
            # We copy pager.uf2 to the mounted volume to automatically restore it!
            time.sleep(2.0)
            os.system("cp dist/pager.uf2 /Volumes/NICENANO/ >/dev/null 2>&1")
            # Wait for reconnect
            wait_for_serial_reconnect(self.PORT, timeout=15)
        except urllib.error.URLError as e:
            self.fail(f"HTTP Bootloader failed: {e}")


if __name__ == "__main__":
    unittest.main()
