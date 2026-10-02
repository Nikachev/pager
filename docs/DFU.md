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
alignment and partition bounds with checked arithmetic. UF2 flags must be exactly
`0x2000`; manifest padding and the final payload length must match the signed
image. Blocks may arrive in any
order and identical duplicates are allowed; conflicting duplicates fail. A
bitmap tracks unique blocks. The final image digest and manifest signature are
verified before completion, and reset is delayed until the successful USB status
has been returned. Unknown or malformed SCSI commands return a failed CSW and an
appropriate sense code.

The volume is `PAGER_BOOT` (current temporary USB identity `239A:0029`). Copy only
`dist/<board>/dev/app/pager.uf2`. `tools/flash_uf2.py` treats neither an I/O error nor early
unmount as success: it waits for the same chip at application identity `1209:0002` and checks
its board, protocol, version and image digest against `package.json`. The host
validates signatures and UF2 structure before rebooting or copying. A copy has
a bounded 90-second deadline and is never automatically retried.
Persistent storage is outside the UF2 range and survives an ordinary update.
An inherited application watchdog is detected and every enabled reload register
is fed while DFU is active. A bootloader panic/HardFault records a retention
reason before reset and enters DFU with the four-long-blink fault code.

```sh
make build
make flash
make flash-swd       # recovery and bootloader replacement
```

## Replace an existing Pager bootloader over USB

Use the dedicated signed updater for a board that already has a Pager bootloader.
The updater runs at the normal Pager application address and replaces only the
48 KiB bootloader partition. It checks chip serial, board, layout, vectors,
embedded digest and NVMC ACL before erasing anything. Pages 1–11 are written
and verified before page 0; application and storage remain outside the write range.
The inherited watchdog is fed throughout. Power interruption protection is outside
this development workflow; keep USB power connected during replacement.

```sh
PAGER_BOARD=nice-nano-v2 PAGER_USB_SERIAL=<16-digit-chip-serial> make update-bootloader
```

The host validates both signed updater and restore app before rebooting. It verifies
the same chip's new `INFO_UF2.TXT` board/version/capabilities and SHA-256 of the
entire bootloader partition, including its erased tail, before restoring the app.
Ordinary `flash_uf2.py` refuses updater packages. There are no automatic transfer
retries. If exact bootloader verification fails, restore is not attempted; use
SWD recovery when USB is no longer available. Keep a programmer available for
first hardware trials; it may remain disconnected until needed.

The on-device updater stops and keeps feeding the watchdog on a validation or
write failure. A preflight failure leaves boot flash intact; a later failure can
require SWD. The factory-XIAO installer below is a different entry path.

## Initial XIAO installation without SWD

A stock Seeed Studio XIAO nRF52840 ships with an Adafruit-compatible UF2
bootloader and S140 v7, whose application starts at `0x00027000`. Pager builds a
one-shot application for that address. The application contains an exact copy
of the XIAO Pager bootloader, erases the factory MBR/SoftDevice, writes and reads
back the Pager bootloader, and resets into `PAGER_BOOT`.
The installer UF2 pads its final payload with `0xFF` to 256 bytes because the
factory bootloader rejects shorter payloads. The host validates this before copying.

The installer accepts only the reviewed bootloader versions 0.6.1/0.6.2 with
S140 7.3.0 on XIAO nRF52840 or Sense. The host checks the version, Board-ID and
SoftDevice in `INFO_UF2.TXT` before copying, requires exactly one matching stock
XIAO on USB, and reads its 16-digit chip serial. Unknown versions, missing
metadata, inaccessible USB descriptors and ambiguous targets are refused.
There is no override for these checks. On Linux, USB access permissions must
allow reading descriptors; `--volume` can select a nonstandard mount path.

The on-device installer reads all eight ACL regions before the first erase. If
any read/write protection intersects `[0, 0x27000)`, it blinks the blue LED and
leaves MBR/SoftDevice intact. Double-press RESET to return to the factory drive;
the uploaded installer has replaced the previous application. This check also
protects against vendor builds that reuse a supported version string but enable
ACL. Newer Adafruit bootloaders protect MBR before handing control to an
application; their ACL cannot be cleared by this installer.

Reviewed handoff sources:
[0.6.1](https://github.com/adafruit/Adafruit_nRF52_Bootloader/blob/0.6.1/lib/sdk11/components/libraries/bootloader_dfu/bootloader_util.c),
[0.6.2](https://github.com/adafruit/Adafruit_nRF52_Bootloader/blob/0.6.2/lib/sdk11/components/libraries/bootloader_dfu/bootloader_util.c).

```sh
# Connect only the target stock XIAO, double-press RESET, wait for its UF2 drive.
make install-xiao

# The installer leaves the board in PAGER_BOOT with no Pager application.
PAGER_BOARD=xiao-nrf52840 make flash
```

The write order keeps factory page zero until last: bootloader pages `1..11`
are written and verified first, the remaining SoftDevice pages are erased, and
then page zero is committed. Nevertheless this is a destructive in-place
migration, not a transactional update. Loss of power during NVMC erase/write may
require SWD recovery. Do not copy `dist/xiao-nrf52840/dev/installer/pager-xiao-installer.uf2` to a board that
has already migrated; it is accepted only by the stock XIAO UF2 layout and is
not a Pager firmware update.

The host reports success only after the saved chip serial enumerates with Pager
VID/PID and product `Pager Boot Drive`. An existing `PAGER_BOOT` volume or another
Pager device does not count. A copy error caused by the factory drive unmounting
is accepted only after this identity check. A timeout is a failure, not proof
that flash stayed unchanged; do not blindly retry a migration after it started.

If macOS reports a copy/flush I/O error and the same chip is still in factory
DFU, reset into factory DFU again before a retry. Do not mix unfinished UF2 and
serial transfers in one bootloader session: a pending flash-page buffer can
corrupt a later serial update. The tested fallback for XIAO Sense 0.6.1/S140
7.3.0 is the Adafruit serial DFU utility. Recheck the board metadata and USB chip
serial first, double-press RESET, then send the installer binary as an
**application** package (never as a bootloader package):

```sh
# Requires adafruit-nrfutil. Replace the port with this board's serial port.
adafruit-nrfutil dfu genpkg --dev-type 0x0052 \
  --application dist/xiao-nrf52840/dev/installer/pager-xiao-installer.bin dist/pager-xiao-installer-dfu.zip
adafruit-nrfutil dfu serial --package dist/pager-xiao-installer-dfu.zip \
  --port /dev/cu.usbmodem1201 --baudrate 115200 --singlebank
```

`Device programmed` confirms the serial transfer only. Migration is successful
only when that chip appears as `Pager Boot Drive` (`239A:0029`) and exposes Pager
metadata on `PAGER_BOOT`.

The development command embeds the local development and release public keys.
For provisioned hardware, `make install-xiao-release` embeds only the release
public key and therefore requires release-signed Pager application updates.

Dev/release trust and downgrade behavior are documented in `SECURITY.md`.


## Development storage schema 7

The current development application stores semantic HID/Battery subscription
flags with the bonds in CRC-protected schema-7 records. Schema 6 is intentionally
incompatible and ignored; the first schema-7 installation starts with Bluetooth
Off and no active bonds/settings. Forget the board on both paired hosts before
re-pairing. This is a logical format reset, not a secure physical erase of old
keys. Ordinary updates within schema 7 preserve user state.

## Transaction and regression checks

The shared `bootloader-core` engine validates a candidate manifest before
committing its block count, so a rejected manifest cannot poison the next transfer.
Each page is erased once per transaction; duplicates require identical readback.
Completion rereads manifest and image from flash and checks signature, digest and
vectors. After a failed MSC transaction the transfer bitmap is reset so an entire
trusted restore can re-erase partially programmed pages. Storage is never writable
through the application transfer range.

MSC retains partial multi-sector read/write positions across polls. A local CBW
sequence identifies a new command even if the host reuses its tag. READ/WRITE(10)
LBA and count are strictly big-endian, with checked virtual disk bounds; short
CDBs and incorrect service actions are rejected. Host CSW validation checks tag,
residue and status. `tools/test_bootloader_framing.py` records malformed signature,
flags and address refusal, omitted/corrupt image recovery and reordered/duplicate
acceptance using an already validated ordinary restore package. A mounted volume
copy error is retained as evidence and never alone establishes success.
