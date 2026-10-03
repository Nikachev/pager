# Pager firmware

Experimental Rust firmware and signed UF2 bootloader for an nRF52840 BLE HID
keyboard with three independently bonded host slots. Supported boards are
nice!nano v2 and Seeed Studio XIAO nRF52840. The USB control page and Bluetooth pairing guide are autonomous HTML files
built from readable sources in `web/`.

## Development status

Pager is in active pre-release development. Flash formats, USB protocol, VID/PID,
pairing data and APIs may change without migration. An incompatible build may
require a full erase and SWD recovery. There is no CI before the first public
release; `make quality` is the required local gate. The current community
VID/PID values are temporary, and selecting final identifiers is a mandatory
release task.

## Build

Install Rust with `thumbv7em-none-eabihf`, `cargo-binutils`, Python 3.14 or later, Node.js for JS tests, and the
packages in `tests/requirements.txt`.

```sh
make build                 # dev-signed app + dist/<board>/dev/app/pager.uf2
make bootloader            # dev bootloader: release + local dev public keys
make build-release         # release-signed app; external private key required
make bootloader-release    # release bootloader: release public key only
PAGER_BOARD=xiao-nrf52840 make build
make quality
```

`PAGER_BOARD` is `nice-nano-v2` by default and may be `xiao-nrf52840`. The
local `make quality` gate runs host tests, Python lint, host and ARM Clippy,
both board builds, bootloader/installer checks, size budgets and generated-file
checks. Hardware checks run separately on the connected board.
The readable UI sources are templates, CSS and JS modules in `web/`.
`xtask build-ui` generates the root HTML pages and their network-free copies in
`dist/ui`; `check-ui` runs behavioral WebUSB/DOM mocks through Node.js.
The first dev build creates an ignored local Ed25519 key pair under `keys/`. Release builds
never use it. See [SECURITY.md](docs/SECURITY.md) and
[RELEASING.md](docs/RELEASING.md).

## Flash

```sh
make flash                 # signed UF2 through PAGER_BOOT
make flash-swd             # bootloader and app through probe-rs
make flash-bootloader      # dev bootloader through probe-rs
PAGER_USB_SERIAL=<serial> make update-bootloader # existing Pager bootloader over USB
make install-xiao          # one-time stock XIAO migration through its UF2 drive
```

Packages are separated by board and signing mode under `dist/<board>/<mode>/`.
Each build preserves its ELF, binaries, UF2 and `package.json` in an immutable
snapshot; the current package link is replaced atomically after validation.

`make flash` validates the package signature, layout and hashes before touching
USB. It succeeds only when the same chip reports the expected board, protocol,
version and image digest. Set `PAGER_USB_SERIAL` when several boards are attached.
Flashing and HIL share an OS-backed lock per chip; a crashed process releases it. Ordinary app
updates do not touch the final 8 KiB storage partition, so Bluetooth enabled
state, active slot, bonds, aliases and the Pager name survive. Factory reset
logically clears current settings and returns to Bluetooth Off. Historical journal
records can retain old keys until page erasure; it is not physical key deletion.

### First install on a stock XIAO nRF52840

Migration without SWD is supported for a XIAO nRF52840 or Sense with a reviewed
Seeed/Adafruit UF2 bootloader version 0.6.1/0.6.2 and S140 7.3.0. Other versions
are refused; this is not a universal installer for every factory bootloader.

1. Connect only the target stock XIAO over USB and quickly press RESET twice.
   Wait for `XIAO-BOOT` or `XIAO-SENSE` to mount.
2. Run `make install-xiao` and do not disconnect power while the blue LED is on.
3. Wait for `PAGER_BOOT`, then run
   `PAGER_BOARD=xiao-nrf52840 make flash` to install the Pager application.

`make install-xiao` is intentionally destructive: its one-shot application runs
at the factory S140 v7 application address, replaces the MBR/SoftDevice with the
Pager bootloader at `0x00000000`, and leaves the board in `PAGER_BOOT`. The
factory bootloader is no longer a recovery path afterward. Use
`make install-xiao-release` when provisioning the release-key-only bootloader.
Before copying, the host checks `INFO_UF2.TXT` and reads the chip USB serial.
The installer checks flash ACL permissions before any erase; success requires
the same chip to enumerate as `Pager Boot Drive`. An ACL refusal blinks the blue
LED without changing MBR/SoftDevice; double-press RESET to return to factory DFU.
See [DFU.md](docs/DFU.md) for failure and recovery details.

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
| factory reset | logically cleared; `Pager`, Bluetooth Off, no active slot |
| storage schema change during development | may be deliberately erased |
| SWD full erase | erased |

Local build layout, version semantics, provenance, offline checks and gates are
documented in [docs/BUILDING.md](docs/BUILDING.md); pinned vendor changes and
required regressions are in [docs/VENDOR_PATCHES.md](docs/VENDOR_PATCHES.md).

Run `make serve-ui` to serve the autonomous USB/Bluetooth pages locally, then open
[USB control](http://localhost:8000/webusb_client.html) in Chrome. See
[WEB_UI.md](docs/WEB_UI.md) for USB permissions and checks.

## Documentation

Operating and development guides live in [docs/](docs/). Work required before
public release is tracked in [RELEASE_TASKS.md](RELEASE_TASKS.md).
Dated validation results and experiment history are kept separately in
[reports/](reports/README.md).

### XIAO L76K GPS

The XIAO firmware supports the Seeed L76K GNSS expansion board on D6/D7.
See [GPS wiring and live diagnostics](docs/GPS.md) for connection and satellite-fix checks.
