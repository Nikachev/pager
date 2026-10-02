# Pager embassy-nrf 0.11.0 patch

Source: crates.io embassy-nrf 0.11.0, upstream commit
3861d3088da30d40c777dc05d282352e68ec5511 (embassy-nrf directory).
Normalized registry Cargo.toml is retained for standalone dependency resolution.
MIT/Apache licenses are retained from that exact commit.

Remove the SIZE.EPOUT write after successful read_dma. ENDEPOUT already allows
another OUT packet; a later SIZE write can discard that unread packet. Initial
endpoint_set_enabled SIZE write is retained. No other driver behavior changes.

References:
- https://github.com/embassy-rs/embassy/pull/6688 (OUT double-arm commit 609e907)
- https://devzone.nordicsemi.com/f/nordic-q-a/35362/usb-bulk-out-hardware-bug
- https://github.com/NordicSemiconductor/nrfx/blob/master/drivers/src/nrfx_usbd.c

Erratum 199 is separate and not included in this minimal packet-loss patch.
Do not claim that this patch alone explains every observed Pager timeout.
Gate: vendor inventory hashes, both ARM builds, protocol host tests, sustained
non-HID full-size OUT+PING integrity, raw framing/events, rendered Mac/Android
HID and app/bootloader UF2 recovery on both boards. Qualified checkpoint07/08
images are unchanged; new packages require their own HIL qualification.
When updating upstream, check read_dma and initial enable separately and repeat
these checks before removing the local patch.

The repository rustfmt gate reformats upstream files; these are separately marked
formatting in PATCHES.json. An explicit list of pre-existing upstream Clippy
lints is allowed in lib.rs, restoring the registry dependency lint treatment
without broadening Pager's own strict gate or rewriting unrelated HAL code.
