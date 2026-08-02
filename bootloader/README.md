# Pager A/B bootloader

The bootloader does not copy a running image. Each 488 KiB bank contains a
4 KiB signed manifest followed by a 484 KiB independently linked application.
It validates both manifests with SHA-256 and Ed25519, then starts a newer image
as a trial. The trial is written to a two-page journal at `0xfc000..0xfe000`
before the jump. The application confirms itself after core initialization; a
reset before that confirmation launches the last confirmed image instead.

The next journal record is always written to the page not holding the current
record, so loss of power while erasing or writing retains a complete earlier
record. Both public signing keys are trusted, enabling a staged key rotation.

Build the bootloader:

```sh
RUSTFLAGS='-C link-arg=-Tlink.x' cargo build --manifest-path bootloader/Cargo.toml --release
```

Create a Slot A package for the first migration:

```sh
make sign SLOT=A VERSION=1
```

`make flash-swd-migration SLOT=A VERSION=1` erases the legacy layout and
installs the bootloader plus the signed Slot A package. Subsequent updates use
the inactive slot: for a confirmed A image, `make flash-http SLOT=B VERSION=2`.

The measured bootloader occupies `0x78c0` bytes, leaving `0x740` bytes inside
the 32 KiB boot partition. Keep SWD recovery available and revisit that budget
before adding material functionality.
