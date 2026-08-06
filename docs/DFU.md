# Single-Slot Secure UF2 Bootloader Architecture (nRF52840)

This document provides a comprehensive technical reference for the **Pager Single-Slot Secure UF2 Bootloader**, memory layout, cryptographic signature verification, Double-Tap reset mechanism, and direct USB Bulk DFU flashing protocol.

---

## 💾 1. Flash Memory Layout

The nRF52840 has 1 MB (1,048,576 bytes) of internal Flash split into 256 pages of 4,096 bytes (4 KB) each.

```
0x0000_0000 +---------------------------------------+ Page 0
            | Bootloader Binary (48 KiB)            |
            | Pages 0..11                           |
0x0000_C000 +---------------------------------------+ Page 12 (FIRMWARE_START)
            | Manifest Header (256 Bytes)          |
            | Magic: "PGRFW001"                     |
            | Ed25519 Signature + SHA-256 Digest    |
0x0000_C100 +---------------------------------------+ (Main Application VTOR Alignment)
            | Main Application (`pager`)            |
            | Pages 12..253 (903.75 KiB)            |
0x000F_E000 +---------------------------------------+ Page 254
            | Protected NVM Storage (8 KiB)         |
            | Pages 254..255                        |
0x0010_0000 +---------------------------------------+ End of Flash (1 MB)
```

---

## 🔑 2. Cryptographic Manifest & Secure Boot Protocol

Every valid application firmware image is prefixed by a 256-byte Manifest Header at address `0x0000_C000`.

### Manifest Header Structure (`ManifestHeader`)
```rust
#[repr(C, packed)]
pub struct ManifestHeader {
    pub state: u32,         // 0xFFFFFFFF (STATE_PENDING)
    pub magic: [u8; 8],     // b"PGRFW001"
    pub version: u32,       // Monotonic Version (1, 2, ...)
    pub image_len: u32,     // Size of raw application binary payload (bytes)
    pub target_slot: u32,   // Reserved (0)
    pub digest: [u8; 32],   // SHA-256 Digest of raw application payload
    pub signature: [u8; 64], // Ed25519 Digital Signature
}
```

### Signature Verification Algorithm
1. The 52-byte signed message is constructed as:
   `magic (8B) + version (4B) + image_len (4B) + target_slot (4B) + digest (32B)`
2. The bootloader validates the Ed25519 signature of `signed_message` against trusted public key(s) in `public_key.rs`.
3. The SHA-256 digest of the application payload (`0x0000_C100 .. 0x0000_C100 + image_len`) is computed and compared against `manifest.digest`.
4. If signature or digest validation fails, execution is halted in DFU mode with LED diagnostic pattern `[3 short blinks] + [2 long blinks]`.

---

## 🔘 3. Double-Tap Reset Mechanism

To allow manual user entry into DFU mode without a software command:
- The bootloader writes a magic flag to uninitialized RAM (`0x2003_FFFC`) on pin reset.
- If a second pin reset occurs within **500 ms**, the double-tap flag is confirmed.
- A fast **15 ms** debounce filter prevents false triggers from mechanical pin noise.
- When double-tap is detected, the bootloader stays in DFU mode and blinks `[3 short blinks]` (User Request).

---

## ⚡ 4. Direct USB Bulk UF2 DFU Protocol

The bootloader exposes a Vendor-Specific USB Bulk Interface (`bDeviceClass = 0xFF`, `bInterfaceClass = 0xFF`) on Endpoint `0x01` (OUT) and Endpoint `0x81` (IN).

### Flashing Sequence
1. Host script (`tools/flash_uf2.py` or `make flash`) sends 512-byte UF2 blocks directly to Endpoint `0x01`.
2. **Block 0 (Manifest Header)**:
   - Received in RAM.
   - Ed25519 signature is verified against trusted public keys in RAM.
   - Page 0 of application Flash (`0x0000_C000`) is erased and Block 0 payload written.
   - Bootloader responds with `0x00` ACK packet on Endpoint `0x81`.
3. **Blocks 1..N (Payload)**:
   - Received in 512-byte chunks, written to Flash sequentially.
   - Bootloader responds with `0x00` ACK packet for each block.
4. **Final Block**:
   - SHA-256 digest of all flashed application blocks is verified.
   - Bootloader issues `SCB::sys_reset()`.
   - Bootloader reboots, validates Flash, sets `SCB->VTOR = 0x0000_C100`, and jumps into main application `pager`.

---

## 🛠️ 5. Commands Summary

```bash
# Build bootloader & signed firmware UF2
make build

# Flash firmware over USB
python3 tools/flash_uf2.py --file dist/pager.uf2

# Flash via SWD Probe (probe-rs)
make flash-swd
```

---

## 🧪 6. Local Verification Workflow

All code validation, lint checks, unit tests, and build verification are performed locally on developer workstations (without external CI runners):

```bash
# Run host unit tests (manifest packing, Ed25519 signature verification)
cargo test --target aarch64-apple-darwin --package xtask

# Run linter checks
cargo clippy --target thumbv7em-none-eabihf -- -D warnings

# Build release bootloader & signed main firmware UF2
make build

# Run automated software DFU reboot & USB enumeration test on hardware
python3 tools/test_dfu_reboot.py
```
