# Local hardware qualification

The prepared fixture is one selected board on USB, slot 1 bonded to the Mac,
slot 2 bonded to an available Android host, slot 3 empty, and radio On. Set
`PAGER_USB_SERIAL` explicitly. Disconnect browser WebUSB before a Python tool
claims the vendor interface. Tools use a chip-specific OS lock released on process
exit; the lock does not coordinate an independently open browser session.

Hardware tests never run as part of the host-only gate. Human preparation covers
board switching, reset/power cycles, pairing and a safe focused text field. Give
exact instructions and obtain a fresh readiness confirmation before dependent
steps. **`test_prepaired_slot_switching_fixture` sends HID text on the Mac**; a
successful protocol response alone is not rendered-text acceptance. Use an empty
text editor with an English keyboard layout, never a command prompt or submit
field. No Enter is needed. VoiceOver qualification is excluded by user decision.

```sh
# After Mac input focus and the fixture are confirmed:
PAGER_USB_SERIAL=<serial> .venv/bin/python -m pytest tests/test_device.py \
  --run-hil -m 'not dfu'

# No HID output; these tools restore the prepared controls:
.venv/bin/python tools/test_usb_framing.py --serial <serial> --output raw.json
.venv/bin/python tools/test_usb_events.py --serial <serial> --output events.json
.venv/bin/python tools/test_usb_integrity.py --serial <serial> --batches 128 --output integrity.json
.venv/bin/python tools/benchmark_usb.py --serial <serial> --samples 100 --output usb.json
.venv/bin/python tools/benchmark_ble_switch.py --serial <serial> --cycles 3 --output ble.json
```

Raw framing covers split/combined packets, ZLP, malformed headers, CRC, garbage
and strict command lengths. Event qualification uses one continuous IN reader,
response correlation, real queue overflow/revision gaps and snapshot recovery;
USB remains live across Off/On and slot switching. Off/On clears the active slot;
choose a slot after enabling. Pairing timeout is 120 seconds and turns the radio Off.

Sustained integrity sends synthetic maximum-size unsupported commands followed
by PING, in batches, with one continuous concurrent IN reader. Every request must
receive the correct response; a framing error from these valid frames is failure.
It sends no HID and checks that durable settings remain unchanged. Preserve the
failure report before recovery; do not turn a successful repeat into a claim that
the original failure never occurred.

Recovery tests are explicitly destructive to the application partition. Keep a
validated ordinary restore and the exact selected serial. They reject updater
packages as a restore and never intentionally cut power during bootloader replacement.

```sh
.venv/bin/python tools/test_bootloader_framing.py --serial <serial> \
  --board <board> --restore <checkpoint>/app/pager.uf2 --output uf2.json
```

Check identity after every update: same chip, board, exact app version/image hash
or bootloader version/partition hash. Compare public durable settings and occupied
slots before/after. Failed transfers retain their logs and are followed only by
a complete trusted restore. Error/unmount/disappearance is not proof of success.

Manual acceptance includes actual rendered text on both hosts, repeated long
256-byte jobs, keyboard dialogs and the standalone UI sequence in [WEB_UI.md](WEB_UI.md).
After factory reset verify the exact fresh state, remove only that board's stale
host entries and pair both slots again. Test cached advertising sets after bond
clear→pairing without controller restart and renamed scan responses. Mac discovery
alone does not establish Android discoverability. A peer identity can occupy only
one slot: pairing the prepared Android host in slot 3 moves its bond out of
slot 2. To preserve both existing bonds, use a third host. With only Mac and
Android available, record this move and restore Android by pairing it again in
slot 2, clear the temporary slot and restore the original device name. Compare
public settings afterward; a new pairing replaces the old encryption key.

Record packages, public key fingerprint, serial, versions/hashes, raw timing
samples, locale, rendered text and recovery outcomes in dated reports
and immutable checkpoints. Compare latency distributions and flash/static RAM
against the same-chip baseline; small samples do not establish a speedup or tail
regression. Static RAM budgets do not measure runtime stack peaks.

macOS owns the bonded HID connection and hides parts of its GATT database from
CoreBluetooth. Those tests have explicit platform skips; rendered input and
notification readiness remain required. Android acceptance is BLE pairing,
reconnect and rendered input on the prepared devices. Android USB OTG, current
measurement, production qualification and release CI are outside this cycle.

Store dated results in `reports/<date>/` and raw artifacts in immutable local
checkpoints. Results belong to their exact recorded images; earlier manual
checks do not qualify a changed build.
