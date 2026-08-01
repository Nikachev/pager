# Testing the pager firmware

This document describes the integration test-suite that exercises the firmware
running on a physically connected **nice!nano v2** (nRF52840) board. The tests
talk to real hardware over BLE, USB-CDC-NCM (HTTP), and USB-CDC-ACM (Serial).

All tests live under `tests/`. The main suite is `tests/test_device.py`
written in native **Pytest** with markers (`smoke`, `dfu`, `ble`), backed by
shared helpers/constants in `tests/common.py`.

---

## Prerequisites

Install Python dependencies on the host:

```bash
pip3 install -r tests/requirements.txt
```

- **macOS** is assumed (for serial port and NCM interface handling).
- The board must be **built first** — tests require `dist/pager.bin` / `dist/pager.uf2` (run `./build.sh`).

---

## Running the suite

From the repository root:

### 1. Run Fast Smoke Tests (< 25s)
```bash
pytest -m smoke -v
```

### 2. Run Heavy Serial DFU Update Test
```bash
pytest -m dfu -v
```

### 3. Run Full Integration Test Suite
```bash
pytest tests/test_device.py -v
```

---

## Test Inventory & Pytest Markers

| Marker | Test Function | Description | Destructive? |
| :--- | :--- | :--- | :--- |
| `smoke` | `test_serial_logs` | Serial log streaming verification | No |
| `smoke` | `test_http_logs` | HTTP `/logs` endpoint & subsystem log markers | No |
| `smoke` | `test_keyboard_state` | GET `/keyboard/state` JSON structure validation | No |
| `smoke` | `test_keyboard_switch` | POST `/keyboard/switch` profile switching | No |
| `smoke` | `test_keyboard_pair` | POST `/keyboard/pair` pairing mode toggle | No |
| `smoke` | `test_keyboard_delete` | POST `/keyboard/delete` bond erasure endpoint | No |
| `smoke` | `test_keyboard_type` | POST `/keyboard/type` text typing emulation | No |
| `smoke` | `test_root_page`..`test_not_found` | GET `/`, `/index.html`, and 404 handler | No |
| `smoke` | `test_update_missing_content_length`..`test_truncated` | HTTP error paths (411, 400, truncated body) | No |
| `smoke` | `test_serial_update_invalid_size` | Serial `update` error path (`ERROR_INVALID_SIZE`) | No |
| `dfu` | `test_serial_update` | Serial DFU firmware self-flashing over CDC-ACM | Yes (reboots) |
| `ble` | `test_ble_functionality` | BLE GATT connection, status notify, LED control | No |
| `ble` | `test_dis_battery_hid_metadata` | BLE DIS, Battery, HID Report Map & Protocol Mode | No |
| `ble` | `test_boot_input_report` | BLE Boot Keyboard Input Report verification | No |
| `ble` | `test_bond_survives_reboot` | Bond slot persistence across reboot | Yes (reboots) |

### Skip behaviour

BLE-dependent tests (`test_ble_functionality`, `test_dis_battery_hid_metadata`, `test_boot_input_report`, `test_bond_survives_reboot`) **auto-skip** when the board is already connected to another BLE host (e.g. the paired macOS keyboard holds the link) or is unreachable.

`test_bond_survives_reboot` additionally skips when **no slot is currently bonded**.

---

## Conventions & Shared Code

`tests/common.py` is the single source of truth for:

- **Hardware constants**: `DEFAULT_PORT` (`/dev/cu.usbmodem123456783`), `DEFAULT_BASE_URL` (`http://192.168.42.1`), `UF2_VOLUME` (`/Volumes/NICENANO`).
- **GATT UUIDs**: `SERVICE_UUID`, `LED_CHAR_UUID`, `STATUS_CHAR_UUID`, `HID_INPUT_REPORT_UUID`, `DIS_SERVICE_UUID`, `BATTERY_SERVICE_UUID`.
- **Helpers**: `run_async()`, `ncm_up/down/ensure_ncm_up()`, `wait_for_http_reconnect()`, `wait_for_serial_reconnect()`, `find_ble_device()`, `raw_http_request()`, `http_request()`, `_expected_hid_report()`.
