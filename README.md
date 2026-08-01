# nice!nano v2 Web Server, BLE & OTA Firmware

Custom bare-metal firmware for the **nice!nano v2** (nRF52840) board running async Rust with **Embassy** and **smoltcp**. This project implements a dynamic HTTP Web Server over a USB-CDC-NCM Ethernet link, featuring safe Web OTA / Serial DFU updates, paired with a BLE GATT HID keyboard emulator using `nrf-sdc`.

---

## 🚀 Key Features

*   **USB-CDC-NCM Networking**: Emulates a USB-Ethernet card. Connects to the host (macOS/Linux) and automatically assigns IP addresses via an embedded DHCP server.
*   **Web Server (smoltcp)**: Hosts a lightweight, self-contained web UI at `http://192.168.42.1/` for diagnostics, slot management, and OTA updates.
*   **Web & Serial DFU Update**: Upload new application binaries through the browser web portal or stream raw bytes over USB CDC-ACM serial (`update <bytes>`). Firmware stages in flash and triggers a soft reboot into the new image.
*   **BLE GATT & HID Keyboard**: Advertises as `nice_nano` and emulates a full Bluetooth Low Energy HID keyboard with 3 profile slots, pairing mode control, and text typing emulation.
*   **Web Bluetooth UI**: A static client webpage (`ble_client.html`) using Chrome/Safari Web Bluetooth API to connect directly to the board over BLE, control the LED, and view live heartbeat logs.

---

## 💾 Memory Layout (nRF52840 Bare-Metal)

| Partition | Start Address | Size | Purpose |
| :--- | :--- | :--- | :--- |
| **Application (Active)** | `0x00000` | 1008 KB (`0xFC000`) | Main application binary running Embassy & BLE stack |
| **Storage & Bonds** | `0xFC000` | 16 KB (`0x04000`) | Flash sector for persistent BLE bond storage |

---

## 🛠️ Prerequisites

Before building, install the standard Rust target and object copy utility:

```bash
# Install the ARM Thumbv7EM compiler target
rustup target add thumbv7em-none-eabihf

# Install cargo-binutils for objcopy tools
cargo install cargo-binutils
```

---

## 📦 How to Build

Simply run the automated build script in the root directory:

```bash
./build.sh
```

This compiles the release profile and outputs the following files in the `dist/` directory:

1.  `dist/pager.bin`: The raw application binary. **Use this for USB Serial DFU updates.**
2.  `dist/pager.hex`: The application Intel HEX image.
3.  `dist/pager.uf2`: Standalone USB UF2 image. **Use this for USB UF2 bootloader flashing.**

---

## ⚡ How to Flash

### 1. Flash via USB Bootloader (UF2)
1. Double-tap the physical reset button on the nice!nano board to enter bootloader mode.
2. Copy `dist/pager.uf2` into the mounted `NICENANO` volume.
3. The board writes the application and reboots automatically.

### 2. Update via USB Serial DFU
1. Open serial connection to `/dev/cu.usbmodem123456783` at 115200 baud.
2. Send command `update <file_size_in_bytes>\n`.
3. Stream the raw bytes of `dist/pager.bin`. The board will flash and reboot automatically.

---

## ⌨️ Bluetooth Keyboard Emulation

This firmware provides full BLE HID Keyboard emulation capabilities, supporting profile management, pairing control, and text typing emulation via HTTP REST endpoints and the Web Portal.

### Profile (Slot) Management
The board maintains 3 separate profile slots in RAM and persistent flash storage to hold bonded devices:

*   **GET `/keyboard/state`**: Retrieves current slot profiles and pairing status.
    *   **Response**: `{"slots":[{"id":0,"active":true,"bonded":false}, ...],"pairing_mode":false}`
*   **POST `/keyboard/switch?slot=<id>`**: Switches active profile slot (`0`, `1`, or `2`) and restarts BLE advertising.
*   **POST `/keyboard/pair`**: Puts active slot into pairing mode to allow new hosts to discover and bond.
*   **POST `/keyboard/delete?slot=<id>`**: Deletes security bond for specified slot.
*   **POST `/keyboard/type`**: Emulates key presses as if typed on a physical keyboard.
    *   **Body**: Raw text to type (up to 128 bytes).

---

## 🧪 Integration Testing

A Pytest integration test suite (`tests/test_device.py`) verifies Bluetooth LE, HTTP endpoints, and Serial commands against a live board.

### Run Fast Smoke Suite (< 25s):
```bash
pytest -m smoke -v
```

### Run Serial DFU Update Test:
```bash
pytest -m dfu -v
```

### Run Full Integration Suite:
```bash
pytest tests/test_device.py -v
```
