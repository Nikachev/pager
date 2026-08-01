# NRF-SDC Migration Analysis

## What was attempted

On the `spike-trouble` branch, `nrf-sdc` (SoftDevice Controller) was linked directly into the firmware binary via `nrf-sdc` + `nrf-mpsl` crates (alexmoon/nrf-sdc rev abe49d22), replacing the legacy Nordic S140 SoftDevice hex blob (`S140_nrf52_7.3.0_softdevice.hex`).

SDC exposes the same Host Controller Interface (HCI) as S140, so the application-level BLE code (`ble.rs`, `main.rs`) needed zero changes — only the build system (Cargo.toml, memory.x) was touched.

## Current state with S140

BLE advertising and bonding work correctly with the S140 hex blob. The test suite (`pytest tests/`) passes.

## What changed in the spike

| File | Change |
|------|--------|
| `Cargo.toml` | Added `nrf-sdc` 0.17.1, `nrf-mpsl` 0.13.0, `trouble-host` git (bt-hci 0.9 compat), `embassy-nrf` 0.11, `embassy-time` 0.4 |
| `memory.x` | Switched to FLASH 0x27000, MPSL STORAGE at 0xFE000, RAM 256K |
| Same source | No BLE/HCI code changes — SDC is a drop-in at the HCI layer |

## Outcome — hard fault on hardware

The binary compiles, links, and flashes via UF2 bootloader, but **hard-faults during MPSL/SDC initialisation** on the nice!nano v2. No advertising ever begins.

## Hypothesis — MBR conflict

MPSL (Nordic's Multiprotocol Service Layer) requires its own MBR (Master Boot Record) at flash address `0x0`. The nice!nano bootloader occupies `0x0` and does not chain-load an MBR. The existing S140 setup avoids this because the S140 hex blob (`S140_nrf52_7.3.0_softdevice.hex`) is flashed as a separate image *after* the bootloader and contains its own MBR at `0x0` — but the bootloader already handles the MBR/SD reset vector forwarding.

With SDC linked in-app, the firmware image must provide the MBR itself (or MPSL's init routine expects one at `0x0`), causing the hard fault. See [sdc/app crate note](https://github.com/alexmoon/nrf-sdc#the-app-crate) — it warns: *"The correct MBR must be linked (mbr_nrf52_3.0.0)."*

## What was missing

- The `mbr_nrf52_3.0.0` crate (or equivalent) was **not** linked
- The `mpsl-mbr-alias` memory region (expected at `0x0`) was **not** configured

## Likely fix path

1. Add `mbr_nrf52 = "3.0.0"` crate + `#![no_main]` entry
2. Add `mpsl-mbr-alias` region in `memory.x` pointing at `0x0` (or forward the bootloader's vector table via a custom MBR shim)
3. Ensure the bootloader still handles DFU entry (may need a custom MBR that checks for DFU magic at boot)

## Recommendation

**Short term — stay on S140.** The hard fault root cause is understood but the fix requires non-trivial MBR/bootloader integration work. The S140 stack is proven, bonded, and passing tests.

**Medium term — revisit SDC when:**
- The nice!nano bootloader is replaced or patched to forward the MBR
- Or a custom MBR shim is written and tested
- Or embassy's HAL provides a turnkey SDC integration for the nice!nano form factor
