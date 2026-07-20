# nice!nano v2 Web Server, BLE & OTA Firmware

Custom firmware for the **nice!nano v2** (nRF52840) board running async Rust with **Embassy** and **smoltcp**. This project implements a dynamic HTTP Web Server over a USB-CDC-NCM Ethernet link, featuring safe Web OTA updates, paired with a BLE GATT control server using the S140 SoftDevice.

---

## 🚀 Key Features

*   **USB-CDC-NCM Networking**: Emulates a USB-Ethernet card. Connects to the host (macOS/Linux) and automatically assigns IP addresses via an embedded DHCP server.
*   **Web Server (smoltcp)**: Hosts a beautiful, responsive web UI at `http://192.168.42.1/` for diagnostics and OTA updates.
*   **Web OTA (Over-the-Air) Update**: Safely upload new application binaries through the browser web portal. The system stages the firmware in secondary flash, validates it, and triggers a soft reboot.
*   **BLE GATT Server**: Advertises as `nice_nano` and runs custom services for LED control and notifications.
*   **Web Bluetooth UI**: A static client webpage (`ble_client.html`) using Chrome/Safari Web Bluetooth API to connect directly to the board over BLE, control the LED, and view live heartbeat logs.

---

## 💾 Memory Layout (SoftDevice S140 v7.3.0)

To maintain compatibility with the Adafruit nRF52 Bootloader and run BLE wireless stacks, the flash and RAM allocations are organized as follows:

| Component | Start Address | Size | Purpose |
| :--- | :--- | :--- | :--- |
| **S140 SoftDevice** | `0x00000` | 156 KB (`0x27000`) | Nordic BLE stack & lower clock handlers |
| **Application (Active)** | `0x27000` | 404 KB (`0x65000`) | Main application binary running Embassy |
| **Staging Partition (OTA)** | `0x8C000` | 404 KB (`0x65000`) | Staging sector for new Web OTA updates |
| **Bootloader & Config** | `0xF1000` | 60 KB | Bootloader code, bonding keys, and settings |

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

This compiles the release profile, merges required Hex blocks, and outputs the following files in the `dist/` directory:

1.  `dist/pager.bin`: The raw application binary. **Use this file for Web HTTP OTA updates.**
2.  `dist/combined.uf2`: The SoftDevice + Application merged file. **Use this for the FRESH / INITIAL flash.**
3.  `dist/pager.uf2`: The standalone application-only UF2. **Use this if SoftDevice is already loaded on the board.**

---

## ⚡ How to Flash

### 1. Fresh / Initial Flash (Via USB Bootloader)
If the board is empty or needs a clean wipe:
1. Double-tap the physical reset button on the nice!nano board.
2. Drag and drop `dist/combined.uf2` into the mounted `NICENANO` volume.
3. The board will write both SoftDevice and the application, and restart automatically.

### 2. Standard Update (Via USB Bootloader)
If SoftDevice is already present:
1. Double-tap the physical reset button.
2. Drag and drop `dist/pager.uf2` to update the application partition without touching SoftDevice.

### 3. Over-the-Air Update (Via Web Browser)
1. Ensure the board is connected via USB and the NCM network link is active.
2. Open `http://192.168.42.1/` in your web browser.
3. In the "Firmware Update (Web OTA)" panel, click **Choose .bin File** and select `dist/pager.bin`.
4. Click **Flash Firmware**. The browser will upload the buffer, and the board will reboot into the new version within 1 second of completion.

---

## 📡 Wireless BLE Control

1. Double-click the included `ble_client.html` file on your host computer (Chrome, Edge, or Opera recommended).
2. Click **Connect to nice!nano** on the page.
3. Select your device `nice_nano` in the pairing popup.
4. Once connected, toggle the LED switch or monitor the incoming heartbeat notifications on the live terminal.

## ⌨️ Bluetooth Keyboard Emulation

This firmware adds full BLE HID Keyboard emulation capabilities, supporting profile management, pairing control, and text typing emulation via HTTP REST endpoints and the Web Portal.

### Profile (Slot) Management
The board maintains 3 separate profile slots in RAM to store bonded devices. You can switch between active slots, trigger pairing mode, or delete bonds dynamically.

*   **GET `/keyboard/state`**: Retrieves the current slot profiles and pairing status.
    *   **Response**: `{"slots":[{"id":0,"active":true,"bonded":false}, ...],"pairing_mode":false}`
*   **POST `/keyboard/switch?slot=<id>`**: Switches the active profile slot (`0`, `1`, or `2`) and restarts BLE advertising.
*   **POST `/keyboard/pair`**: Puts the active slot into pairing mode to allow new devices to discover and bond with the keyboard.
*   **POST `/keyboard/delete?slot=<id>`**: Deletes the security bond for the specified slot.
*   **POST `/keyboard/type`**: Emulates key presses as if typed on a physical keyboard.
    *   **Body**: Raw text to be typed (up to 128 bytes of UTF-8 text).
    *   **Translation**: Automatically maps ASCII characters to HID standard codes and key modifiers.

### Web Interface Control
The built-in web portal at `http://192.168.42.1/` includes a glassmorphic **Bluetooth Keyboard Controller** panel that allows you to:
1.  Monitor connection/bond status of all 3 slots in real-time.
2.  Switch profiles and initiate pairing for the active slot.
3.  Type text in a text box and send it to the board, which instantly replicates the typing action on the connected Bluetooth host.

---

## 💻 Device Management (HTTP & Serial)

The firmware supports retrieving logs, rebooting to the bootloader, and updating firmware via both HTTP and USB Serial interfaces.

### HTTP Interface (Web UI)
*   **Live Debug Logs**: Served at `http://192.168.42.1/logs`. The web page at `http://192.168.42.1/` automatically polls this endpoint every 2 seconds to display logs in real-time.
*   **Reboot to Bootloader**: Triggered by sending a `POST` request to `http://192.168.42.1/bootloader`, or by clicking the **Reboot to Bootloader** button on the web portal.
*   **Web OTA Update**: Handled via `POST /update` with the binary file.

### USB Serial Interface (CDC-ACM)
When connected, the board streams logs in real-time. You can connect to the serial port (e.g. `/dev/cu.usbmodem123456803` on macOS) at `115200` baud.
*   **Reboot to Bootloader**: Send `bootloader` or `dfu` to the serial port.
*   **Serial Firmware Update**:
    1. Send `update <size_in_bytes>\n` to the serial port.
    2. Wait for the `SERIAL_UPDATE:READY` log message.
    3. Stream exactly `<size_in_bytes>` raw binary bytes to the serial port.
    4. Upon successful completion, the board writes the firmware, prints `SERIAL_UPDATE:SUCCESS`, and reboots.

---

## 🧪 Integration Testing

A comprehensive integration test suite is available in `tests/test_device.py` to verify Bluetooth LE, HTTP endpoints, and Serial commands against a physically connected board.

### Prerequisites
Install the required Python testing packages on the host:
```bash
pip3 install pyserial bleak
```

### Running Tests
Execute the test runner:
```bash
python3 -m unittest -v tests/test_device.py
```
*Note: During bootloader reboot tests, the test suite automatically resets the board back to application mode by copying the UF2 binary to the mounted `/Volumes/NICENANO` volume, ensuring the board is not left in bootloader mode.*

