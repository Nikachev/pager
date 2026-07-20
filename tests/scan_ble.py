import asyncio
from bleak import BleakScanner

async def main():
    print("Scanning for BLE devices...")
    devices = await BleakScanner.discover(timeout=10.0)
    for d in devices:
        name = d.name or "Unknown"
        print(f"[{name}] {d.address}")

if __name__ == "__main__":
    asyncio.run(main())
