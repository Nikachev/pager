# Signed single-slot UF2 update

The 1 MiB flash layout comes only from `layout.json`: a 48 KiB bootloader,
application partition beginning at `0x0000C000`, and two protected 4 KiB storage
pages at `0x000FE000`. The first 256 bytes of the application partition hold the
`PGRFW002` manifest; the vector table begins at `0x0000C100`.

The 112-byte manifest contains magic, numeric version, image length, SHA-256
digest and Ed25519 signature. The signed message is the first 48 bytes (everything
except the signature). The remainder of the 256-byte manifest area is `0xFF`.

The bootloader accepts 512-byte UF2 sectors carrying 256 payload bytes. It checks
all magic values, nRF52840 family ID, block count/index, address, payload length,
alignment and partition bounds with checked arithmetic. Blocks may arrive in any
order and identical duplicates are allowed; conflicting duplicates fail. A
bitmap tracks unique blocks. The final image digest and manifest signature are
verified before completion, and reset is delayed until the successful USB status
has been returned. Unknown or malformed SCSI commands return a failed CSW and an
appropriate sense code.

The volume is `PAGER_BOOT` (current temporary USB identity `239A:0029`). Copy only
`dist/pager.uf2`. `tools/flash_uf2.py` treats neither an I/O error nor early
unmount as success: it waits for the application identity `1209:0002` afterward.
Persistent storage is outside the UF2 range and survives an ordinary update.
An inherited application watchdog is detected and every enabled reload register
is fed while DFU is active. A bootloader panic/HardFault records a retention
reason before reset and enters DFU with the four-long-blink fault code.

```sh
make build
make flash
make flash-swd       # recovery and bootloader replacement
```

Dev/release trust and downgrade behavior are documented in `SECURITY.md`.
