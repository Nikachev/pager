# Dependency update: XIAO hardware verification

Date: 2026-10-03. Board: xiao-nrf52840, USB serial `ED9F6FD799C4C309`.
Evidence: `dist/checkpoints/dependencies-20261003/xiao-nrf52840/` (local, Git-ignored).
Power: USB only, no battery (user confirmed).

Application: `26.10.1-03115913`, SHA-256
`a84dfa967d9b7485c045a4426b30381d8bfd57d732726f56871e54449b362da0`.
Bootloader: `0.3.0-20261003130230`, partition SHA-256
`bcbbb27c2e3ccbcc8aa728be8e51bb040e6f0da8bfbf597590c5ec125a6b58cf`.
Signed chip-bound bootloader update and application restore passed; settings and
both existing bonds were preserved.

## Results

- USB framing: all 24 cases passed.
- USB events: interleaving, overflow, snapshot recovery, Bluetooth Off/On and
  switching with live USB passed. An initial run stopped at its slot-1 fixture
  precondition before running checks; selecting slot 1 allowed the test to run.
- USB integrity: 128 batches, 4,096 correct responses, settings preserved.
- UF2: all six refusal/recovery cases passed (signature, flags, storage address,
  missing final block, digest, reordered/duplicate blocks); candidate restored.
- Device HIL: four passed, two macOS GATT tests skipped, two DFU tests deselected
  because the six-case UF2 run covers recovery separately.
- Mac fixture marker manually confirmed. Long HID: four jobs per host,
  782 characters; Mac copy exactly matched, Android manually confirmed without
  visible omissions. User clarified the duplicate chat paste was not HID duplication.
- Fresh pairing of a second Android in slot 3 passed with both original public
  bond identities preserved.
- First cached re-pair attempt did not establish a durable bond; Android showed
  paired while the board still advertised with an empty slot. The window expired
  and radio turned Off. A retry after enabling radio succeeded, but does not
  qualify the no-controller-restart path. A further controlled clear/rename/re-pair
  without toggling Bluetooth succeeded as `PagerXiaoCached 3`, with a stored bond,
  encryption and HID readiness; both original public identities stayed unchanged.
  All attempts are preserved. The initial failure remains unexplained.
- Temporary slot 3 cleared; original `PagerXiao` name and public fixture restored.
  Cold power persistence passed: reset reason 0, exact image and all public
  settings preserved. Both hosts reached HID readiness and acknowledged their
  short cold-start markers; user confirmed both rendered exactly on the correct host.

USB timing samples are retained in `baseline-usb.json` and `candidate-usb.json`;
no speedup is claimed from these small sequential samples.
The full earlier `make quality` gate passed for both boards. No firmware source
changes were made during this hardware run.

## Final fixture and remaining observation

Candidate application and bootloader remain installed; Bluetooth enabled,
`PagerXiao` base name, original Mac in slot 1 and Android in slot 2, slot 3 empty,
active slot 2. Cold-start evidence: `cold-boot-result.json`.
The update's planned hardware checks are complete. The initial failed cached
pairing attempt remains an unresolved observation; subsequent successful attempts
do not erase it. No claim of universally reliable pairing follows from this run.
