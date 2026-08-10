# Pager firmware

Experimental Rust firmware and signed UF2 bootloader for an nRF52840 BLE HID
keyboard with three independently bonded host slots. Supported boards are
nice!nano v2 and Seeed Studio XIAO nRF52840. The WebUSB and Web Bluetooth pages
are standalone files and intentionally share only their visual design.

## Development status

Pager is in active pre-release development. Flash formats, USB protocol, VID/PID,
pairing data and APIs may change without migration. An incompatible build may
require a full erase and SWD recovery. There is no CI before the first public
release; `make quality` is the required local gate. The current community
VID/PID values are temporary, and selecting final identifiers is a mandatory
release task.

## Build

Install Rust with `thumbv7em-none-eabihf`, `cargo-binutils`, Python 3 and the
packages in `tests/requirements.txt`.

```sh
make build                 # dev-signed app + dist/pager.uf2
make bootloader            # dev bootloader: release + local dev public keys
make build-release         # release-signed app; external private key required
make bootloader-release    # release bootloader: release public key only
PAGER_BOARD=xiao-nrf52840 make build
make quality
```

`PAGER_BOARD` is `nice-nano-v2` by default and may be `xiao-nrf52840`. The
local `make quality` gate compiles both board variants; HIL currently runs on
nice!nano v2, so XIAO remains compile-validated until matching hardware is used.
The readable standalone UI sources are the two HTML files at the repository
root. `xtask build-ui` publishes their network-free artifacts to `dist/ui`
without requiring Node.js.
The first dev build creates an ignored local Ed25519 key pair under `keys/`. Release builds
never use it. See [SECURITY.md](docs/SECURITY.md) and
[RELEASING.md](docs/RELEASING.md).

## Flash

```sh
make flash                 # signed UF2 through PAGER_BOOT
make flash-swd             # bootloader and app through probe-rs
make flash-bootloader      # dev bootloader through probe-rs
```

`make flash` succeeds only after the application re-enumerates. Ordinary app
updates do not touch the final 8 KiB storage partition, so Bluetooth enabled
state, active slot, bonds, aliases and the Pager name survive. Factory reset
erases all user settings and returns to Bluetooth Off.

## Memory layout

`layout.json` is the source of truth for generated linker scripts and build-tool
checks.

| Partition | Address | Size |
| --- | --- | ---: |
| Bootloader | `0x00000000` | 48 KiB |
| Signed manifest + application | `0x0000C000`–`0x000FDFFF` | 968 KiB |
| Persistent storage | `0x000FE000`–`0x000FFFFF` | 8 KiB |

The single-slot `PGRFW002` manifest contains signed numeric version, image
length and SHA-256 digest. Anti-rollback is deliberately not implemented yet;
valid older signed firmware is accepted.

## Tests

Host tests do not require hardware. HIL is launched manually after preparing two
paired hosts: slot 1 is the Mac running tests, slot 2 is another device. From that
point the run requires no human action and never clears pairing. Typing on the
second host is a separate manual acceptance test. See
[HIL_TESTING.md](docs/HIL_TESTING.md).

Protocol and recovery details are in [usb-protocol.md](docs/usb-protocol.md) and
[DFU.md](docs/DFU.md). The USB trust boundary is physical access: a connected host
may change settings, slots and keyboard output, but firmware still requires a
trusted signature.

## Support and persistence matrix

| Path | Current scope |
| --- | --- |
| Board | nice!nano v2; Seeed Studio XIAO nRF52840 |
| USB control/UF2 | macOS; Android USB OTG deferred |
| BLE HID | current macOS and latest Android/Chrome |
| Automated HIL | local macOS, two pre-paired hosts |

| Operation | Bonds, aliases, name, radio/active slot |
| --- | --- |
| reset / power cycle | preserved |
| ordinary `make flash` | preserved |
| factory reset | erased; `Pager`, Bluetooth Off, no active slot |
| storage schema change during development | may be deliberately erased |
| SWD full erase | erased |
