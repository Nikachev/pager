# Pager bootloader

Synchronous nRF52840 USB MSC bootloader for signed, single-slot, out-of-order UF2
updates. Layout is generated from the repository `layout.json`; validation and
transaction details are in `docs/DFU.md`.

`cargo` defaults to the dev variant, which trusts the repository release key and
the machine-local dev key. Build the production trust set explicitly through
`make bootloader-release`; it contains only the release public key. A signed chip-bound updater replaces a working Pager bootloader through USB;
SWD is recovery when USB is unavailable. A stock XIAO nRF52840 may
perform its first migration without SWD through `make install-xiao`; see
`docs/DFU.md` at the repository root.

The shared `bootloader-core` owns manifest/UF2/layout/vector parsing, the mockable
flash engine and partial-sector cursor. `msc_flash` adapts them to NVMC and the
virtual disk. READ/WRITE(10) uses strict big-endian LBA/count, local command
identity and bounded buffering. Core mock-flash and storage transport tests run
on the host. ARM builds enforce flash and static RAM budgets; the 16 KiB RAM
margin is a budget reserve, not a measured stack maximum. Current boot flash
headroom is small: verify the size gate for every change.

## nRF USB driver patch

The bootloader uses the same pinned local `embassy-nrf` dependency as the app.
The OUT DMA completion path omits a second `SIZE.EPOUT` write that can discard
the next unread packet. Initial enable retains its SIZE write. See
[`PAGER_PATCH.md`](../vendor/embassy-nrf/PAGER_PATCH.md) for provenance and checks.
Changes to this driver require signed updater and UF2 recovery qualification;
existing checkpoint bootloader images are not modified by source edits.
