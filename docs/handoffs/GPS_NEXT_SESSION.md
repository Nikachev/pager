# GPS / XIAO handoff — 2026-10-03

## Resume here

The user approved implementing and executing the remaining plan **only at the
existing window position**. They explicitly rejected testing outdoors. Work was
stopped before any new implementation of the startup measurement plan: the last
turn only read `tools/gps_diagnostics.py`, `src/gps_task.rs`, and
`src/runtime/clock.rs`. Do not assume startup timestamps or a three-run harness
already exist.

Repository: `/Users/nikachev/github/pager`, branch `main`. All changes from this
GPS work are uncommitted, including new untracked source files; keep them.
There is no separate worktree or PR. Do not reset/clean this checkout.

## User constraints and authorization

- L76K is connected only to XIAO. Current target XIAO chip serial:
  `A222C62566851775` (USB serial may appear lowercase).
- There is **no external L76K backup supply**, and none is planned soon.
  Do not assume Seeed's expansion board battery/backup circuit is installed or
  functioning merely because it appears in generic documentation.
- Flash stores only very rarely changing settings. GPS position, UTC anchors,
  last-fix timestamps, orbital data and time zones stay in RAM. No new storage
  schema or periodic GPS flash writes.
- Antenna/board stayed at the same window position through previous measurements.
  No outdoor comparison is authorized or requested.
- User approved 3 cold starts at that position plus loss-of-signal behavior checks.
  Physical removal/restoration of power requires their participation. A software
  reset or application update resets XIAO RAM but does not necessarily power-cycle
  L76K. Do not call it a cold start.
- Do necessary implementation/build/testing autonomously. Ask user to perform
  power cycles only once the recorder is ready, starting observation beforehand.

## Already implemented

- XIAO UART: 9600 8N1, RX D7/P1.12, TX D6/P1.11.
  Buffered UART uses UARTE0, TIMER2, PPI 0/1, group 0, RX buffer 1024 bytes.
  nice!nano does not initialize GPS peripherals.
- `src/gps.rs`: bounded checksum-valid NMEA parser, GGA coordinates in signed
  microdegrees, RMC active-status date/time validation, latest valid raw sentence.
- `src/clock.rs`: host-testable UTC clock corrected from RMC; monotonic holdover.
  `src/runtime/clock.rs`, publicly re-exported as `crate::clock` in firmware:
  `now_utc_ms() -> Option<u64>`, `sync_age_ms()`. UTC unknown before a valid
  correction; no persistence; GPS serial latency limits accuracy (no PPS discipline).
- `src/gps_task.rs`: current status, RAM last-known position/capture UTC/age,
  bounded 24-packet RAM NMEA history with age. No GPS flash writes or background
  coordinate logs.
- WebUSB v5 additive commands in `protocol.json`:
  `get_gps` = 14, no arguments; `get_gps_sentence` = 15, index byte 0–23.
  Latest history first; missing entry returns empty response. History is not an
  atomic full GSV cycle. Response limit remains 512 bytes.
- USB page: coordinates, board UTC converted to GPS-derived estimated IANA zone,
  DST handled via Intl; Russian date format `ДД.ММ.ГГГГ, ЧЧ:ММ:СС`.
  Compact copy icon next to longitude copies BOTH current coordinates in
  `latitude, longitude` form for Google Maps. Disabled for historic coordinates.
  RAM last-known coordinates labeled with age when fix lost; UTC holdover shown.
- `web/gps_view.js`, `web/usb_app.js`, template/CSS; embedded offline geographic
  lookup @photostructure/tz-lookup 11.7.0 in `web/vendor/` with licenses/provenance.
  No coordinates sent to external services. Geographic lookup is approximate.
- Readable sources build autonomous `webusb_client.html` and `dist/ui/` via
  `tools/build_ui.py`; `tools/generate_protocol.py` regenerates checked-in contracts
  and page. Edit sources, then regenerate; do not hand-edit generated HTML.
- `tools/test_gps.py`: live stream test, `--require-fix`, `--wait-device`.
- `tools/gps_diagnostics.py`: raw status/history capture once per 2 s. Currently
  requires board already connected; duration measured after USB connection, not
  module power-on. Writes JSON only at normal completion; improve this for
  long-running/disconnect-safe measurement.

## Latest installed firmware and evidence

Last installed, signed-UF2/digest-verified application:
`26.10.2-03152718`, target chip above.
Package: `dist/xiao-nrf52840/dev/app/pager.uf2` (may be replaced by a later build).
Reports: `reports/2026-10-03/GPS_L76K.md`; instructions: `docs/GPS.md`.
Local Git-ignored evidence: `dist/checkpoints/gps-20261003/`.

Earlier 180-second observation after user-confirmed power cycling: valid serial
NMEA, no errors, GGA satellites used=0 and UTC unsynchronized. This did NOT prove
no visible satellites. Later 30-second GSV capture at the SAME window position
showed fixes in all 15 samples, final 7 satellites used, GPS UTC synchronized,
13 GPS + 2 BeiDou satellites in view, nonempty CN0 10–34 dB-Hz in the final cycle.
GSA reported 3D fix, with high HDOP 22.1 in an inspected packet. Root cause of the
long reacquisition delay remains unproved; exact TTFF was not measured because
first later sample already had a fix. Do not infer a settings fault or definitive
RF fault. Receiver settings were not changed or queried; backup rail not measured.

Raw evidence includes `cold-start.{json,log}`, `gsv.{json,log}`,
`gsv-summary.json`, `clock.json`, `clock-ui.json`, `ram-installed.json`.
Keep precise coordinates in local evidence rather than public report prose.

Previously passed: 55 Rust library tests, Clippy both boards, 20 JS behavior
checks, 92 Python tests (8 HIL skips) in an earlier full run; recent scoped Python
USB/protocol suite 14 tests. Latest strict command bounds test also passed.
Physical RAM retention after losing a previously acquired fix is still pending;
the historic display path has a JS test.

## Remaining approved plan

1. Add RAM timestamps at firmware startup for first received byte, first valid
   NMEA, first GSV reporting view/signal, first valid GGA fix, first valid RMC UTC.
   Retain first-event times even when the short history rolls over. Expose a
   bounded diagnostics response separately if needed: avoid overflow of existing
   512-byte GPS status. Distinguish firmware monotonic startup baseline from
   physical power-on (USB/bootloader introduces delay). Do not claim exact
   power-on TTFF from a late host poll.
2. Build a host recorder armed before power restoration, selecting chip strictly,
   waiting for removal/reconnection, capturing status/GSV/GSA/RMC and milestone
   times. Handle USB disconnects and bounded waits; flush incremental local
   evidence so interruption does not lose samples. Use `hardware_lock`.
3. Verify parser/timestamp/recorder logic with meaningful tests; build and install
   XIAO app, retain nice!nano compile compatibility. Prepare everything BEFORE
   asking for the first physical cycle.
4. Coordinate 3 full power-off runs with the user, all at the original window.
   Remove every MAIN supply to XIAO/L76K (USB and main battery if connected).
   Start observing before user reconnects; record actual limits/uncertainty on
   power-on timing. Choose a reasonable bounded acquisition timeout and record
   non-fixes as failures/timeouts, never as made-up TTFF values.
5. After a fix, coordinate temporary signal loss without powering off. Confirm
   board UTC keeps advancing in holdover; coordinates are labeled last-known;
   copy current coordinates disabled. Then restore reception and check correction.
   Do not use a GNSS restart command as a substitute for RF signal loss.
6. Verify power loss clears RAM position/UTC and update report with all 3 outcomes.
   Use measured results to decide whether any startup settings/AGNSS work is
   justified; do not add aiding or change receiver configuration speculatively.

## Useful commands

```sh
PAGER_BOARD=xiao-nrf52840 make clippy
PAGER_BOARD=nice-nano-v2 make clippy
cargo test --locked --target aarch64-apple-darwin --lib
node --test tests/ui_behavior.cjs
.venv/bin/python -m pytest -q tests
.venv/bin/python tools/generate_protocol.py
.venv/bin/python tools/build_ui.py
PAGER_BOARD=xiao-nrf52840 make build
.venv/bin/python tools/flash_uf2.py --file dist/xiao-nrf52840/dev/app/pager.uf2 \
  --board xiao-nrf52840 --serial A222C62566851775
.venv/bin/python tools/gps_diagnostics.py --serial A222C62566851775 \
  --duration 30 --output dist/checkpoints/gps-20261003/next.json
```

Use `pytest tests`, not bare repository-wide `pytest`: tools and tests include
same-named modules and cause collection collisions. Firmware flash and diagnostics
must not compete for the hardware lock.

The UI was served at http://127.0.0.1:8000/webusb_client.html from `dist/ui`.
A Python HTTP server may still be running; check before starting a second one.
In-app browser native USB chooser was not automatable via available tools;
read-only hardware calls and shipped JS were verified separately. Avoid wasting
work repeating that limitation; the user can connect manually for UI HIL.
No automated hardware recorder is intentionally left running for this handoff.

Official protocol reference:
https://raw.githubusercontent.com/Seeed-Projects/Seeed_L76K-GNSS_for_XIAO/fb74b715224e0ac153c3884e578ee8e024ed8946/docs/Quectel_L76K_GNSS_Protocol_Specification_V1.1.pdf
Seeed wiring/reference: https://wiki.seeedstudio.com/get_start_l76k_gnss/

## Update after executing this handoff (2026-10-04 local)

The startup measurement implementation and all three user-confirmed full main
power cycles above ARE NOW COMPLETE; do not repeat implementation or assume
runs are pending. Installed/digest-verified XIAO app: `26.10.2-03204522`.
`get_gps_startup` command 16 returns first RAM events in firmware monotonic ms.
`tools/gps_recorder.py` captures serial-selected, locked JSONL with fsync,
bounded waits and transport reconnection. First byte is UART read time, not
physical wire edge. No GNSS configuration/restart/aiding or flash schema change.

First GGA fixes: run 1 = 818359 ms, run 2 = 268614 ms, run 3 = 131452 ms;
first RMC UTC = 818822 / 268999 / 131812 ms. All three initial samples showed
unknown UTC and no RAM last-known position. All acquired within 900 s after USB
application connection, no subsequent loss samples, no UART/NMEA errors.
Raw evidence and summaries: `dist/checkpoints/gps-20261003/run-{1,2,3}*`.
Do not claim exact physical-power-on TTFF.

Subsequent steel-pot RF shielding at the same position did NOT cause loss:
279 samples, zero loss/holdover, final 9 satellites. Entire record duration
563.13 s includes baseline/coordination, not a precise covered interval.
User says board rests on plastered hollow ceramic on fourth floor; no bottom
shield/insulator available and adding one requires movement. Do not ask again
to add a metal bottom or relocate without new user direction. Physical holdover,
last-known UI/copy behavior and reacquisition correction remain unverified.
Recorder deliberately stopped; no GPS hardware recorder is running.
Evidence: `signal-loss*` in same checkpoint directory. Report updated.
User may remove the pot with power maintained; no further hardware action
is currently armed. Keep all uncommitted/untracked work in this checkout.

## Final update — measurement plan complete (2026-10-04 local)

Supersedes the pending RF-loss items above. User authorized moving indoors,
then moved board with laptop without interrupting USB power. Real fix loss
observed: zero satellites, valid NMEA, holdover. Measured UTC advance 50542 ms
matched 50542 ms monotonic over 26 samples; unchanged RAM last position with
increasing age. Agent inspected connected browser UI: last-known label/age,
clock advance (7 s), last-location zone label, copy button disabled. Page USB
connection was released before a restoration recorder began.
User returned board to original window; current position and GPS UTC correction
resumed without reset. Restored fix stable for 66.815 s, final 7 satellites,
GPS correction age 86 ms; no UART/NMEA/transport errors. Recorders stopped.
Evidence `room-loss*`, `room-restore*` in checkpoint directory; report updated.
All approved stages are complete. Do not repeat or treat RF checks as pending.
Acquisition variation remains unexplained; no speculative GNSS config/aiding
changes made. UI is disconnected but can be reconnected by the user. Preserve
all uncommitted/untracked changes. No further physical action is armed.
