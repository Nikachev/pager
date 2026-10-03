# Pager tests

`make test` runs Rust and Python host tests without hardware. `make quality-fast`
adds formatting/lint, host Clippy and the offline vendor inventory check.
`make quality` also checks both ARM applications, bootloaders, updater and factory
installer packages, signatures, partition/flash/static RAM budgets, generated
protocol/golden vectors and autonomous UI behavior. Run with `--locked`; cached
dependencies permit `CARGO_NET_OFFLINE=true` and a fresh `CARGO_TARGET_DIR`.

`node --test tests/ui_behavior.cjs` runs the actual JS session/app modules with
USB and DOM mocks. It covers lifecycle, asynchronous cancellation, pending cleanup,
request wrap, malformed frames/state, coalesced refresh, UTF-8 validation, busy
controls, bounded logs and dialog/focus behavior. It does not prove rendered HID
or actual browser accessibility.

Hardware is opt-in and uses a chip-specific POSIX `flock`, released by the OS
when the owner exits. Set `PAGER_USB_SERIAL`; close browser WebUSB. Prepare slot
1=Mac, slot 2=Android and slot 3=empty. **The prepared-slot contract test sends
HID text** and requires a freshly confirmed safe focused Mac field. See
[HIL_TESTING.md](../docs/HIL_TESTING.md) for exact commands and manual acceptance.
A missing device or incorrect fixture is failure, not a skip. macOS GATT access
has explicit platform limitations because the system owns HID.

Markers are `hil`, `smoke`, `contract`, `ble`, and `dfu`; destructive tests require
`--run-hil --run-destructive`. DFU checks validate a trusted restore and the same
chip's expected identity. `tools/test_bootloader_framing.py` preserves malformed
transfer failures and complete recovery evidence. Storage/runtime fault images
are development-only, explicitly prepared and separate from ordinary HIL.

`tools/test_usb_integrity.py` checks sustained full-size OUT traffic with a
concurrent IN reader, matching every synthetic command/PING response and checking
settings afterward. It is opt-in, serial-bound and sends no HID. The local nRF
USB driver patch requires this check in addition to framing/events and rendered
HID on both boards; the vendor hash inventory alone is not hardware evidence.

The gate includes caller-owned stream-buffer lifecycle tests and exhaustive
digest formatting comparison for all byte values. Record hardware evidence and
exact image scope separately in `reports/<date>/`.
