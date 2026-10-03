# Dependency update: nice!nano hardware verification

Date: 2026-10-03. Board: nice-nano-v2, USB serial `ECA27894EBB268AC`.
Evidence: `dist/checkpoints/dependencies-20261003/nice-nano-v2/` (local, Git-ignored).

Application: `26.10.1-03115907`, SHA-256
`24091c2a6967149aab3235ab35ca0c7e51b5855450eab583f0a9dee0be3769f3`.
Bootloader: `0.3.0-20261003120932`, partition SHA-256
`13fb97cd711ab374d753342235cb5b2cd6b89c325621067dcfde0ceceda7794f`.
The signed chip-bound updater and application restore succeeded.

## Results

- USB framing: 24 cases passed. Event overflow observed as expected.
- USB integrity: 4,096 correct responses; settings preserved.
- Device HIL: four passed; two GATT tests skipped on macOS. UF2 tested separately.
- UF2: six rejection/recovery cases passed, including signature, flags, address,
  missing block, digest, and reordered/duplicate blocks. Candidate restored.
- Bonded Mac/Android switching and Bluetooth Off/On passed.
- Fresh slot-3 pairing and renamed cached advertising/re-pairing succeeded
  without restarting the board: Android discovered `PagerAuditAgain 3`.
- Existing peer deduplication moved Android's bond from slot 2 to slot 3 during
  this test. Android was then paired again in slot 2; Mac's original bond stayed.
  Final public settings match the starting fixture; Android's key was replaced.
- Initial Android long HID output was manually confirmed by block/end markers.
  Initial Mac output began with a Russian layout; keyboard-position normalization
  matched the expected text except the omitted trailing space in the pasted copy.
- Cold power cycle: USB-only power disconnected/reconnected by the user;
  diagnostic reset reason changed from 4 to 0. Image and public settings match.
  The USB observer missed the disconnect, so it supplies no independent USB edge proof.
- After cold start both hosts acknowledged four HID jobs (14 + 256 + 256 + 256
  = 782 characters). HID-ready waits were 4,929 ms on Mac and 5,327 ms on Android.
  User-pasted Mac output exactly matches all 782 expected characters, including
  the final space. Android output was manually confirmed without visible omissions.

## Reconnect finding

One candidate Mac reconnect exceeded a 30-second readiness timeout before any
HID was sent; it subsequently reconnected. This remains an observed issue.
Controlled old/new firmware comparisons on the same updated bootloader showed
variable Android tails on both versions. With the Android screen confirmed on,
new/old/new Android samples were 12.447/6.392 s, 6.419/20.637 s, and
8.991/9.343 s respectively. These small samples do not establish a stable
firmware-specific regression or prove the timeout resolved. All attempts remain
in the evidence directory. USB latency stayed comparable across versions.

## Final fixture and scope

Candidate application is installed, Bluetooth enabled, `PagerNano` base name,
Mac in slot 1, Android in slot 2, slot 3 empty, active slot 2.
This report covers nice!nano only; the later [XIAO run](DEPENDENCY_HIL_XIAO.md)
records that board separately.
No firmware source changes were made during this run. The earlier full
`make quality` gate passed (164 Rust, 92 Python, 17 UI tests and both-board builds).

## Independent third-host pairing

A second Android paired in slot 3 as `PagerThird 3`, then successfully paired
again as `PagerThirdAgain 3` after clearing only slot 3 and renaming advertising
without a board restart. Both existing Mac/first-Android public bond identities
remained unchanged across both pairings; all three slots reached bonded state
and slot 3 reached HID readiness. The base name was restored to `PagerNano`.
Evidence: `three-host/{before,fresh-pair-result,cached-pair-result,three-bonds-restored-name}.json`.
Two rounds of switching (slots 1 → 2 → 3) acknowledged all six HID jobs. The
user confirmed each host received exactly its own two markers, without omissions
or text intended for another host. Ready waits were 4.943–14.384 seconds. Slot 3
was then cleared and slot 2 selected; the original public fixture was restored.
Evidence: `three-host/{switching-hid,restored}.json`.
