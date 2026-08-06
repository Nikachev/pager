# Pager Single-Slot Secure UF2 Bootloader

The bootloader occupies the first 48 KiB (`0x00000 .. 0x0C000`) of Flash memory on nRF52840.
It validates the Ed25519 digital signature and SHA-256 digest of the manifest header (`PGRFW001`) at `0x0C000`, verifies the vector table at `0x0C100`, and jumps directly into the main application.

---

## 🔑 Key Features
- **Single-Slot Memory Map**: Application partition from `0x0C000` to `0xFE000` (904 KiB total).
- **Ed25519 & SHA-256 Validation**: Blocks unsigned or corrupted images from executing.
- **Double-Tap Reset Trigger**: Double-pressing the reset button within 500 ms forces entry into DFU mode.
- **Vendor-Specific USB Bulk DFU**: Fast direct USB Bulk endpoints for UF2 flashing via `python3 flash_uf2.py`.

---

## 🔨 Building the Bootloader

```sh
cargo build --manifest-path bootloader/Cargo.toml --release
```

---

## ⚡ Flashing
Use `make flash-swd` to write both the release bootloader and signed application via SWD probe.
