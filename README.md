# nice!nano v2 Web Server, BLE & OTA Firmware

Custom bare-metal firmware for the **nice!nano v2** (nRF52840) board running async Rust with **Embassy** and **embassy-net**. It exposes a USB-CDC-NCM HTTP control plane, signed staged firmware updates, and experimental BLE HID work built on `nrf-sdc`.

---

## 🚀 Key Features

*   **USB-CDC-NCM Networking**: Emulates a USB-Ethernet card. Connects to the host (macOS/Linux) and automatically assigns IP addresses via an embedded DHCP server.
*   **Web Server (smoltcp)**: Hosts a lightweight, self-contained web UI at `http://192.168.42.1/` for diagnostics, slot management, and OTA updates.
*   **A/B Web & Serial DFU**: Upload a signed image into the inactive bank, boot it once as a trial, and automatically restore the confirmed bank if it resets before confirmation.
*   **BLE GATT & HID Keyboard**: Advertises as `TrouBLE-Pager` and emulates a full Bluetooth Low Energy HID keyboard with 3 profile slots, pairing mode control, and text typing emulation.
*   **Web Bluetooth UI**: A static client webpage (`ble_client.html`) using Chrome/Safari Web Bluetooth API to connect directly to the board over BLE, control the LED, and view live heartbeat logs.

---

## 💾 Memory Layout (nRF52840 Bare-Metal)

| Partition | Start Address | Size | Purpose |
| :--- | :--- | :--- | :--- |
| **Bootloader** | `0x00000` | 32 KiB (`0x08000`) | Signature verification, A/B selection, trial and rollback |
| **Slot A** | `0x08000` | 488 KiB | 4 KiB manifest + 484 KiB image at `0x09000` |
| **Slot B** | `0x82000` | 488 KiB | 4 KiB manifest + 484 KiB image at `0x83000` |
| **Boot-control journal** | `0xFC000` | 8 KiB | Two-page power-loss-safe confirmed/trial record |
| **Storage & bonds** | `0xFE000` | 8 KiB | Persistent BLE profile and bond state |

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

## 📦 Build & Flash via Makefile

A standard `Makefile` is available for building, flashing, and testing:

```bash
# 1. Build a release image for one slot
make build SLOT=A

# 2. Flash via HTTP OTA (Default method)
make flash               # or: make flash-http

# 3. Flash via USB Serial DFU (Backup/Reserve method)
make flash-serial

# 4. Flash via SWD Probe (Hardware Recovery)
make flash-swd

# 5. Run HIL Integration Tests
make hil                 # or: make test
```

You can override defaults via environment variables or a local `.env` file (see `.env.example`):
```bash
cp .env.example .env
```
Or directly from CLI:
`make flash-http SLOT=B DEVICE_IP=192.168.42.1`
`make flash-serial SLOT=B PORT=/dev/cu.usbmodem123456783`

---

## ⚡ How to Flash

### 1. Flash via HTTP OTA (Default)
Upload the application binary directly over the USB-Ethernet HTTP link:
```bash
make flash-http SLOT=B
```
The uploader supplies `X-Pager-CRC32` and uploads `dist/pager-B.signed.pkg`; use `make flash-http` rather than a bare `curl` command.

### 2. Flash via USB Serial DFU (Backup/Reserve)
Stream binary chunks over USB serial interface:
```bash
make flash-serial SLOT=B
```
Or manually: `python3 scripts/flash_serial.py /dev/cu.usbmodem123456783 dist/pager-B.signed.pkg`

### 3. Flash via SWD Probe (Hardware Recovery)
If the board is unresponsive or unbricking is required, program directly over SWD using `probe-rs`:
```bash
make flash-swd SLOT=A
```

After a SWD reset, unplug and reconnect the board directly to USB before trying HTTP OTA. macOS must enumerate the new CDC-NCM interface before `192.168.42.1` is reachable.

> OTA validates CRC-32 in transit, then SHA-256 and Ed25519 in the bootloader. The manifest names its physical target slot. The bootloader records a trial before launch; the application confirms only after core initialization. A reset before confirmation rolls back to the preceding confirmed slot.

### Release procedure

For development, `make sign` uses the current Unix time as a convenient version.
For a release, choose a strictly increasing unsigned 32-bit release number and
pass it explicitly; do not rely on a build machine clock:

```bash
make sign-release SLOT=B RELEASE_VERSION=42
make verify-package
```

Keep private keys outside the repository. The bootloader currently trusts the
current and next public keys: install a bootloader containing a new key before
signing packages with it, and remove an old key only in a later SWD migration.

### First A/B migration via SWD

This explicitly destructive operation erases the old single-bank firmware and
BLE bonds, then installs the A/B bootloader and a signed Slot A image:

```bash
make flash-swd-migration SLOT=A VERSION=1
```

With Slot A confirmed, the first OTA goes to Slot B:

```bash
make flash-http SLOT=B VERSION=2
```

`GET /health` reports `slot` and `ota_target_slot`. Always build the next
package for `ota_target_slot`; after confirmed Slot B, use `SLOT=A`.

### Rollback HIL runbook

For a destructive local rollback check, load a higher-version image that does
not confirm its trial, reset it through CDC, then verify the preceding slot:

```bash
# From confirmed Slot B, create a deliberately unconfirmed Slot A trial.
make flash-http SLOT=A VERSION=3 TRIAL_NO_CONFIRM=1
printf 'reboot\n' > /dev/cu.usbmodem123456783
# Reconnect USB if macOS keeps the NCM interface inactive.
curl http://192.168.42.1/health  # slot must again be 1
```

`TRIAL_NO_CONFIRM=1` is an HIL-only fixture and must never be used for a
normal firmware release. A higher-version normal package for that failed slot
is accepted as an explicit retry; it replaces the stale trial record.

---

## ⌨️ Bluetooth Keyboard Emulation

This firmware provides full BLE HID Keyboard emulation capabilities, supporting profile management, pairing control, and text typing emulation via HTTP REST endpoints and the Web Portal.

The board's HTTP address is intentionally not a secure browser origin, so Chrome
cannot expose Web Bluetooth to a page served from `192.168.42.1`. To use the
optional GATT control client, serve it locally (Chrome trusts `localhost`):

```bash
make ble-client
# Open http://localhost:8000/ble_client.html in Chrome.
```

The chooser filters by Pager's advertised custom service UUID, avoiding stale
entries retained from older development BLE addresses.

### Profile (Slot) Management
The board maintains 3 separate profile slots in RAM and persistent flash storage to hold bonded devices:

*   **GET `/keyboard/state`**: Retrieves current slot profiles and pairing status.
    *   **Response**: `{"slots":[{"id":0,"active":true,"bonded":false}, ...],"pairing_mode":false}`
*   **POST `/keyboard/switch?slot=<id>`**: Switches active profile slot (`0`, `1`, or `2`). The new profile is used on the next BLE connection.
*   **POST `/keyboard/pair`**: Puts active slot into pairing mode to allow new hosts to discover and bond.
*   **POST `/keyboard/delete?slot=<id>`**: Deletes security bond for specified slot.
*   **POST `/keyboard/type`**: Emulates key presses as if typed on a physical keyboard.
    *   **Body**: Raw text to type (up to 128 bytes).
*   **GET `/health`**: Returns OTA status plus dropped log/BLE-command counters for diagnosis.

---

## 🧪 Integration Testing

A Pytest integration test suite (`tests/test_device.py`) verifies Bluetooth LE, HTTP endpoints, and Serial commands against a live board.

### Run Fast Smoke Suite (< 20s):
```bash
pytest -m smoke -v
```

Run the extended non-destructive endpoint and error-path checks separately:

```bash
pytest -m contract -v
```

### Run Serial DFU Update Test:
```bash
pytest -m dfu -v
```

DFU tests erase and reboot the board, so they require an explicit opt-in:

```bash
make test-dfu
# or: pytest -m dfu -v --run-destructive
```

### Run Full Integration Suite:
```bash
pytest tests/test_device.py -v
```
