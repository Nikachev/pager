# Testing the pager firmware

This document describes the integration test-suite that exercises the firmware
running on a physically connected **Pager** (nRF52840) board. The tests
talk to real hardware over WebUSB, USB-CDC-ACM (Serial), and BLE.

All tests live under `tests/`. The main suite is `tests/test_device.py`
written in native **Pytest** with markers (`smoke`, `dfu`, `ble`), backed by
shared helpers/constants in `tests/common.py`.

---

## Prerequisites

Install Python dependencies on the host:

```bash
pip3 install -r tests/requirements.txt
```

- **macOS** or **Linux** with PyUSB and WebUSB support.
- The board must be **signed first** — destructive update tests require a package
  for the inactive A/B slot (for example `make sign SLOT=B VERSION=2`).

---

## 🔒 Hardware Concurrency Lock

To prevent concurrent test processes from colliding on the single physical nRF52840 board, `tests/conftest.py` and `scripts/flash_webusb.py` enforce an automatic process lock using `/tmp/pager_hil_test.lock`. If another test process or DFU flash script is currently active on the device, secondary runs exit immediately with a clear message:
```text
[-] Error: Another HIL test process is already running on the physical device. Concurrent runs are forbidden.
```

---

## Running the suite

From the repository root via Makefile or Pytest:

### 1. Run Fast Smoke Tests (< 20s)
```bash
make test-smoke          # or: pytest -m smoke -v
```

### 2. Run DFU Firmware Update Tests (WebUSB & Serial DFU)
```bash
make test-dfu            # or: pytest -m dfu -v --run-destructive
```

DFU tests are intentionally skipped unless `--run-destructive` is supplied,
because they overwrite the running image and reboot the board.

### 3. Run Full Integration Test Suite
```bash
make hil                 # or: make test / pytest tests/test_device.py -v
```

---

## Test Inventory & Pytest Markers

| Marker | Test Function | Description | Destructive? |
| :--- | :--- | :--- | :--- |
| `smoke` | `test_serial_logs` | Serial log streaming verification | No |
| `contract` | `test_serial_update_invalid_size` | Serial `update` error path (`ERROR_INVALID_SIZE`) | No |
| `dfu` | `test_webusb_update` | WebUSB OTA into the inactive A/B slot and trial reboot | Yes (reboots) |
| `dfu` | `test_serial_update` | Serial OTA into the inactive A/B slot and trial reboot | Yes (reboots) |
| `ble` | `test_ble_functionality` | BLE GATT connection, status notify, LED control | No |
| `ble` | `test_visible_gatt_metadata_and_hid_when_exposed` | Visible DIS metadata; HID details when CoreBluetooth exposes service 0x1812 | No |

---

## Conventions & Shared Code

`tests/common.py` is the single source of truth for:

- **Hardware constants**: `DEFAULT_PORT` (`/dev/cu.usbmodem*`).
- **GATT UUIDs**: `SERVICE_UUID`, `LED_CHAR_UUID`, `STATUS_CHAR_UUID`, `HID_INPUT_REPORT_UUID`, `DIS_SERVICE_UUID`, `BATTERY_SERVICE_UUID`.
- **Helpers**: `run_async()`, `wait_for_serial_reconnect()`, `wait_for_serial_disconnect()`, `find_ble_device()`.
