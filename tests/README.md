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
- The board must be **signed first** — destructive update tests require a package
  for the inactive A/B slot (for example `make sign SLOT=B VERSION=2`).

---

## 🔒 Hardware Concurrency Lock

To prevent concurrent test processes from colliding on the single physical nRF52840 board, `tests/conftest.py` and `scripts/flash_serial.py` enforce an automatic process lock using `/tmp/pager_hil_test.lock`. If another test process or DFU flash script is currently active on the device, secondary runs exit immediately with a clear message:
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

### 2. Run Extended Non-Destructive Contract Tests

```bash
pytest -m contract -v
```

### 3. Run DFU Firmware Update Tests (HTTP OTA & Serial DFU)
```bash
make test-dfu            # or: pytest -m dfu -v --run-destructive
```

DFU tests are intentionally skipped unless `--run-destructive` is supplied,
because they overwrite the running image and reboot the board.

For the first A/B migration and rollback proof, follow the destructive
[`Rollback HIL runbook`](../README.md#rollback-hil-runbook). It verifies a
normal Slot A → Slot B trial/confirmation, an intentionally unconfirmed trial,
and return to the prior confirmed slot.

### 4. Run Full Integration Test Suite
```bash
make hil                 # or: make test / pytest tests/test_device.py -v
```

---

## Test Inventory & Pytest Markers

| Marker | Test Function | Description | Destructive? |
| :--- | :--- | :--- | :--- |
| `smoke` | `test_serial_logs` | Serial log streaming verification | No |
| `smoke` | `test_http_logs` | HTTP `/logs` endpoint & subsystem log markers | No |
| `smoke` | `test_health` | HTTP `/health` queue-pressure and OTA status | No |
| `smoke` | `test_root_page`..`test_not_found` | GET `/`, `/index.html`, and 404 handler | No |
| `contract` | `test_keyboard_*` | Keyboard profile and input endpoint contract | No |
| `contract` | `test_update_missing_content_length`..`test_truncated` | HTTP error paths (411, 400, truncated body) | No |
| `contract` | `test_update_missing_checksum`, `test_update_bad_checksum` | OTA CRC-32 integrity contract | No |
| `contract` | `test_serial_update_invalid_size` | Serial `update` error path (`ERROR_INVALID_SIZE`) | No |
| `dfu` | `test_http_update` | HTTP OTA into the inactive A/B slot and trial reboot | Yes (reboots) |
| `dfu` | `test_serial_update` | Serial OTA into the inactive A/B slot and trial reboot | Yes (reboots) |
| `ble` | `test_ble_functionality` | BLE GATT connection, status notify, LED control | No |
| `ble` | `test_dis_battery_hid_metadata` | BLE DIS, Battery, HID Report Map & Protocol Mode | No |
| `ble` | `test_boot_input_report` | BLE Boot Keyboard Input Report verification | No |
| `ble` | `test_bond_survives_reboot` | Bond slot persistence across reboot | Yes (reboots) |

### Skip behaviour

BLE-dependent tests (`test_ble_functionality`, `test_dis_battery_hid_metadata`, `test_boot_input_report`, `test_bond_survives_reboot`) skip only when no matching Pager is discoverable. A discovered device that fails to connect is a test failure.

On macOS, `test_bond_survives_reboot` actively requests pairing by accessing the
encrypted HID input report. Just Works pairing completes automatically; when
macOS displays a system pairing sheet, accept it (or let Computer Use accept
that local sheet). One Mac can validate pairing, slot selection and persistence,
but it cannot emulate three independent Bluetooth identities. Full isolation of
three separately bonded profiles remains a multi-host HIL test.

`test_bond_survives_reboot` additionally skips when **no slot is currently bonded**.

---

## Conventions & Shared Code

`tests/common.py` is the single source of truth for:

- **Hardware constants**: `DEFAULT_PORT` (`/dev/cu.usbmodem123456783`), `DEFAULT_BASE_URL` (`http://192.168.42.1`).
- **GATT UUIDs**: `SERVICE_UUID`, `LED_CHAR_UUID`, `STATUS_CHAR_UUID`, `HID_INPUT_REPORT_UUID`, `DIS_SERVICE_UUID`, `BATTERY_SERVICE_UUID`.
- **Helpers**: `run_async()`, `ncm_up/down/ensure_ncm_up()`, `wait_for_http_reconnect()`, `wait_for_serial_reconnect()`, `find_ble_device()`, `raw_http_request()`, `http_request()`, `_expected_hid_report()`.
