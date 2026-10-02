# Persistent storage v7

Pager deliberately rejects earlier development schemas. The first v7 boot uses
canonical fresh state: name `Pager`, Bluetooth Off, no selected slot and no bonds.
Ordinary v7 app updates preserve settings. Pairing state is never durable.

Two layout-defined 4 KiB pages form an append-only copy-on-write journal. Each
aligned 276-byte record contains a 40-byte header, three 76-byte slot records,
CRC-32 and a final four-byte commit word. `PGS7` and schema 7 identify the record.
The header contains selected slot/none, Bluetooth enabled, wrapping sequence,
a 24-byte UTF-8 device name and three semantic CCCD flag bytes. CCCD bits are
Report Input=1, Boot Keyboard Input=2 and Battery Level=4; transient attribute
handles and subscription-table layouts are not stored.

An occupied slot contains address kind/address, optional IRK, LTK, security
level, bonded flag and a 32-byte UTF-8 alias. IRK absence is explicitly tagged
zero, rather than the erased value 255. Empty slots carry no current security
data. A trusted bond with both HID input flags missing is healed once by
restoring Report and Boot Keyboard notification flags.

One complete scan chooses state, cursor and cached encoded durable record from
the newest valid wrapping sequence. Read errors are reported rather than treated
as empty storage. Startup retries briefly; persistent read failure enters USB
bootloader recovery without erasing settings. The persistence task uses the
startup cursor, so these selections cannot disagree across separate scans.

An unchanged encoded snapshot is already durable and skips erase/write. An
append writes its body and CRC before the commit word, then verifies the entire
record by readback before advancing the cursor or publishing success. A partly
programmed slot is skipped on retry. At rollover, the opposite page is erased;
the last committed record on the current page remains available if that erase
or the following write is interrupted.

Persistence requests have wrapping sequence IDs. A snapshot acknowledges only
requests captured with it. A bounded terminal batch history retains failure
ranges across later successes; evicted results fail closed. Rollback invalidates
also pending mutations discarded during flash/recovery awaits. Duplicate signals
cannot turn an earlier failure into success. Waiters independently observe the
history with one five-second deadline.

On write failure, runtime settings reload the durable state. If recovery reads
also fail, the last verified cached record is used. BLE controller/session
restart restores radio and subscriptions from those settings; USB remains live.
A reported failure can still have applied an effect if a commit completed before
a later I/O failure. Clients must reread state after an error or timeout.

Factory reset is a **logical append-reset**: it clears current bonds, aliases and
settings, selects name `Pager` and disables Bluetooth. Historical records can
still contain old keys until journal page erasure. It does not promise physical
key deletion. An explicitly opted-in development fault image additionally
supports storage erasure: both pages are erased and verified before fresh state
is committed. That control is unavailable in normal builds and forbidden in
release builds. See `tools/test_runtime_faults.py` and the implementation journal
for controlled fault qualification.

Qualification distinguishes fresh-state validation from update preservation.
Ordinary app and bootloader updates compare public durable state before/after;
factory reset must match the exact canonical state. Bond material stays out of
reports. A successful pair is verified independently by an occupied slot,
connected profile and HID notification readiness, then rendered text on the host.
