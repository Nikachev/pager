# Pager bootloader

Synchronous nRF52840 USB MSC bootloader for signed, single-slot, out-of-order UF2
updates. Layout is generated from the repository `layout.json`; validation and
transaction details are in `docs/DFU.md`.

`cargo` defaults to the dev variant, which trusts the repository release key and
the machine-local dev key. Build the production trust set explicitly through
`make bootloader-release`; it contains only the release public key. Changing keys
or recovering a broken bootloader requires SWD.
