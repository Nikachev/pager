# Vendor source and patch inventory

`vendor/PATCHES.json` inventories every vendored file, including removed upstream
files, with upstream and installed SHA-256 hashes. `tools/check_vendor.py` verifies
the local files and license hashes offline in `quality-fast` and `quality`. It
rejects changed, missing, or undeclared files. Update the manifest deliberately
when changing a vendor patch; the checker never rewrites it.

| Package | Exact upstream | Local semantics |
| --- | --- | --- |
| Trouble host 0.8.0 | [42e3e04](https://github.com/embassy-rs/trouble/tree/42e3e04de00db3951b126741e1f3602bfdf5df47/host) | Runtime SMP/on-air address synchronization; persistent advertising-set handles and prepared-set re-enable. |
| Trouble host macros 0.6.0 | [Same commit](https://github.com/embassy-rs/trouble/tree/42e3e04de00db3951b126741e1f3602bfdf5df47/host-macros) | No semantic patch; formatting only. |
| usbd-storage 3.0.0 | [f2015e7](https://github.com/apohrebniak/usbd-storage/tree/f2015e7554cec0a4f1c5433b1730f01bf96cbdf8/usbd-storage) | Pager SCSI/command compatibility, START STOP/SYNCHRONIZE CACHE, strict CDB validation, descriptor simplification, processed-byte accounting, and local CBW generations. Upstream BOT 3.0 supplies short-transfer and bounded-read handling. |
| embassy-nrf 0.11.0 | [3861d30](https://github.com/embassy-rs/embassy/tree/3861d3088da30d40c777dc05d282352e68ec5511/embassy-nrf) | Remove the second OUT endpoint arm after DMA completion; retain initial enable. Applies to application and bootloader. |

Trouble Rust files have formatting changes from the upstream 120-column/2024
configuration to this repository's 100-column/2021 formatting, plus a trimmed
comment whitespace change. After normalizing formatting, the semantic Trouble
diffs are limited to `lib.rs` and `peripheral.rs`; see
`vendor/trouble-host/PAGER_PATCH.md`. Both upstream licenses are preserved as
`vendor/LICENSE-MIT` and `vendor/LICENSE-APACHE`.

usbd-storage retains its MIT license and `.cargo_vcs_info.json` source commit.
The copied registry Cargo manifest/lock and package markers differ from the Git
source and are declared in the inventory. UFI and examples are excluded; Pager
only compiles SCSI/BOT.
Registry integration tests are excluded because they target the unmodified
command API; upstream BOT unit tests and Pager CDB/accounting tests are retained.
Dependency compatibility edits are in the manifest.
The READ/WRITE(10) endian heuristic was removed during inventory review: all LBA
and count fields now use SCSI big-endian decoding. Truncated CDBs and unsupported
READ CAPACITY(16) service actions return Unknown; the Pager adapter rejects virtual
disk ranges beyond its advertised capacity before I/O.

Trouble's runtime address setter keeps SMP identity synchronized with the
selected slot's on-air address without changing the controller-global address.
Persistent advertising handles preserve the slot-to-set mapping; prepared-set
re-enable avoids rewriting identity/parameters while still refreshing advertising
and scan-response data. Upstream 0.8.0 does not provide these runtime operations.
Its security mutex feature gates already support peripheral-only security, so
Pager carries no local workaround for those gates.

## Updating a dependency

1. Fetch the exact proposed upstream revision into a separate checkout; retain
   the current packages, source, licenses, and recovery checkpoint.
2. Compare complete source trees against the declared old revision. Normalize
   formatting separately so semantic and packaging/subset changes stay visible.
3. Reapply only the required semantic patches; update source commits and all
   upstream/local file hashes in `vendor/PATCHES.json` and preserve licenses.
4. Run the complete local gate and the board-specific hardware matrix below.
   Rebuild chip-bound bootloader updaters if the MSC/flash code changed.

Required regression coverage: Trouble security/peripheral/gatt/derive unit tests,
application two-host bonded switching, Off/On, disconnect/reconnect, CCCD/HID
readiness, prepared-set advertising/scan-data refresh after rename and bond
clear→pairing without controller restart, and rendered HID acceptance; usbd-storage partial destination reads,
large big-endian LBA/count and every truncated CDB, partial multi-sector retries,
exact CSW status/residue/tag checks, and application UF2 malformed/incomplete/
reordered/duplicate refusal and recovery on both boards. Bootloader replacement
uses the signed chip-bound updater before testing the new implementation.

The pinned nrf-sdc/nrf-mpsl revision is now
`e44e619d15eba9145453e3b2d7b07121556871ab`; its central+peripheral feature workaround
is separate from Trouble's peripheral-only role and is not removed by this patch.

The nRF HAL is copied from the published 0.11.0 crate, with its normalized Cargo
manifest and exact Git provenance. Licenses are retained in `vendor/embassy-nrf`.
Formatting and specific pre-existing upstream Clippy compatibility changes are
declared separately from the USB fix. See its `PAGER_PATCH.md` for primary sources.
The independent Erratum 199 workaround is not part of this minimal patch.
Required hardware coverage includes sustained valid maximum-size OUT/PING
integrity with concurrent IN reads, framing/events, rendered HID on both hosts,
and signed updater plus UF2 recovery on both boards. Root Cargo patches the
transitive HAL dependencies too, so BLE and application peripherals use one
crate identity.
