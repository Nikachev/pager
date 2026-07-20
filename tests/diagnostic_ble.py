import asyncio
import urllib.request
import urllib.error
import time
import sys
from bleak import BleakScanner, BleakClient

# Configuration
KEYBOARD_IP = "192.168.42.1"
DEVICE_NAME = "nice_nano_4"
HID_SERVICE_UUID = "00001812-0000-1000-8000-00805f9b34fb"
INPUT_REPORT_UUID = "00002a4d-0000-1000-8000-00805f9b34fb"

def trigger_pairing_mode():
    print(f"[*] Triggering Pairing Mode on the board via HTTP POST http://{KEYBOARD_IP}/keyboard/pair...")
    try:
        req = urllib.request.Request(f"http://{KEYBOARD_IP}/keyboard/pair", method="POST")
        with urllib.request.urlopen(req, timeout=5) as res:
            if res.status == 200 and res.read().decode('utf-8') == "Success":
                print("[+] Successfully activated Pairing Mode! Board is advertising...")
                return True
    except Exception as e:
        print(f"[-] Failed to trigger pairing mode: {e}")
        print("    Make sure the NCM network link is active and http://192.168.42.1/ is reachable.")
    return False

async def run_diagnostic():
    print("==================================================")
    print("      BLE Keyboard Pairing Diagnostic Tool        ")
    print("==================================================")

    # 1. Trigger Pairing mode
    if not trigger_pairing_mode():
        print("[-] Aborting due to HTTP control channel failure.")
        return

    # Wait a second for advertising to restart
    await asyncio.sleep(2.0)

    # 2. Scanning
    print(f"\n[*] Scanning for '{DEVICE_NAME}' Bluetooth advertisement...")
    device = None
    try:
        # Scan for 10 seconds
        devices = await BleakScanner.discover(timeout=10.0)
        for d in devices:
            name = d.name or "Unknown"
            print(f"    Found device: {name} (Address: {d.address})")
            if name == DEVICE_NAME or name == "nRF5x" or d.address.upper() == "3D181354-5E69-3C55-D6A9-238E4BE8F041":
                device = d
    except Exception as e:
        print(f"[-] Scan failed with error: {e}")
        return

    if not device:
        print(f"\n[-] Device '{DEVICE_NAME}' NOT found in scan results.")
        print("    Possible causes:")
        print("    1. Board is already connected to another device (BLE only advertises when disconnected).")
        print("    2. Bluetooth on this laptop is disabled or lacks permissions.")
        print("    3. Board is out of range or not powered.")
        return

    print(f"\n[+] Found '{DEVICE_NAME}' (Address: {device.address})")

    # 3. Connection
    print(f"\n[*] Connecting to {device.address}...")
    try:
        async with BleakClient(device) as client:
            print("[+] Connection established!")
            print(f"    MTU: {client.mtu_size}")

            # 4. Service Discovery
            print("\n[*] Discovering services and characteristics...")
            services = client.services
            hid_service = None
            for service in services:
                print(f"    Service: {service.uuid} ({service.description})")
                if service.uuid.lower() == HID_SERVICE_UUID:
                    hid_service = service
                for char in service.characteristics:
                    print(f"      Characteristic: {char.uuid} ({char.description}) - Properties: {char.properties}")

            if not hid_service:
                print("[-] HID Keyboard Service (0x1812) not found in GATT database!")
                return

            # 5. Trigger Pairing/Bonding
            print(f"\n[*] Triggering pairing by reading the protected Input Report characteristic ({INPUT_REPORT_UUID})...")
            print("    (On macOS, reading a protected characteristic triggers the OS to start secure bonding).")
            try:
                # Sleep a second before reading to let discovery settle
                await asyncio.sleep(1.0)
                data = await client.read_gatt_char(INPUT_REPORT_UUID)
                print(f"[+] Read successful! Characteristic Value: {data}")
                print("[+] Pairing and bonding completed successfully!")
            except Exception as e:
                print(f"[-] Read failed (expected if pairing dialog was rejected or timed out): {e}")
                print("\n    If a macOS system dialog prompted you to pair, did you accept it?")
                print("    If no dialog appeared, macOS may have blocked it due to stale keys.")

    except Exception as e:
        print(f"[-] Connection failed with error: {e}")

if __name__ == "__main__":
    if sys.platform == "win32":
        asyncio.set_event_loop_policy(asyncio.WindowsSelectorEventLoopPolicy())
    asyncio.run(run_diagnostic())
