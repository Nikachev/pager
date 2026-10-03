# XIAO L76K verification

Date: 2026-10-03. Chip: `A222C62566851775`. Board: XIAO nRF52840.
Installed application: `26.10.2-03143410`.
Image SHA-256: `3a5d1f4198d519b0aa7c80d6c725602084d42100b9a3710f74acfee0f9b86777`.

L76K reception uses D7/P1.12, UART 9600 8N1, with D6/P1.11 wired as TX.
Pin mapping and factory UART settings were checked against Seeed's
[L76K documentation](https://wiki.seeedstudio.com/get_start_l76k_gnss/) and
[XIAO pin map](https://wiki.seeedstudio.com/XIAO-BLE-Sense-Pin-Multiplexing/).

Signed UF2 installation and running board/version/digest verification passed.
The live GGA connection test passed. A subsequent 180-second satellite-fix
check sampled 179 responses. NMEA counter advanced from
370 to 2160, bytes from 13246 to 80504.
Final checksum/parser errors: 0; UART overruns: 0.
The final NMEA age was 340 ms; GGA age 694 ms.

The user confirmed the antenna was attached and moved it toward a window.
Positioning did **not** pass: final fix quality 0, satellites
0, fix=0. Clean reception confirms module power and
the module-to-XIAO UART path. Antenna reception and satellite positioning
remain unverified; repeat `tools/test_gps.py --require-fix --timeout 180`
with clear sky visibility. No claims are made about the unused TX command path.

Validation: Rust library suite (52 tests),
three GPS parser tests including repeated GGA, corrupt packets, missing fields,
fix loss and overflow recovery; protocol argument validation; 14 Python
protocol/USB tests; Clippy with warnings denied for both boards; Python Ruff;
generated protocol consistency and whitespace checks.

Raw evidence is local and Git-ignored:
`dist/checkpoints/gps-20261003/{connection.json,fix.json,fix.log}`.

## Follow-up: coordinates, UTC clock and USB page

Installed application `26.10.2-03145106` adds signed microdegree coordinates,
a shared board UTC source (`crate::clock::now_utc_ms`) and RMC correction.
The module subsequently obtained a live fix with **7 satellites**, quality 1.
Three consecutive USB samples reported fresh coordinates and `time_source=gps`,
with advancing UTC milliseconds and zero UART/parser errors. This supersedes
the earlier no-fix limitation above: satellite positioning and GPS time are now
confirmed on this board.

The actual captured replies were decoded by the shipped page's `PagerGps`
module. All resolved to `Europe/Sofia`; board UTC was displayed as
`03/10/2026, 17:53:15` local time for the last sample. Precise location evidence
remains in Git-ignored local JSON, not this report.

Validation: 55 Rust tests, Clippy for XIAO and nice!nano, 19 JavaScript behavior
tests, and 92 Python tests passed (8 hardware tests skipped). Clock tests cover
calendar validation, fractional seconds, leap-year dates, correction, holdover
and midnight. UI tests cover GPS-derived zones, DST, fractional-hour offsets,
stale coordinates/time, unsynchronized time and disconnect cleanup.

The new panel was visually inspected in the in-app browser while disconnected.
The browser automation could not operate its native USB chooser, so the live
browser USB connection was not verified end-to-end; hardware replies and their
shipped UI decoding/rendering were verified separately. The page is available
at localhost:8000/webusb_client.html for manual connection.

Local evidence: `dist/checkpoints/gps-20261003/{clock.json,clock-ui.json}`.
Timezone data: pinned @photostructure/tz-lookup 11.7.0, checked npm SHA-512;
geographic lookup is approximate. The board retains UTC, while the page applies
browser Intl time-zone/DST rules. No GPS coordinates leave the local page.

## No-backup configuration and cold-start observation

The user clarified there is no external backup supply and instructed that flash
hold only very rarely changing data. GPS fixes/time/time zones are therefore
never persisted. The new application retains last valid position, monotonic
age and optional UTC capture time only in RAM; firmware reset clears it.
The USB page labels last-known coordinates and disables copying them as current.
The existing storage layout and BLE persistence path are unchanged.

After requesting full removal of power to XIAO and L76K, the user replied `+`.
A 180-second observation of the previous application received
1925 new valid NMEA sentences, with
0 parser/checksum errors and 0 UART errors.
No fix arrived: satellites=0, time_source=unsynced.
This confirms clean serial reception and unsynchronized board time during
acquisition; it does not establish TTFF under adequate satellite reception.
The power-off interval was performed by the user, not measured by host software.
Evidence: `dist/checkpoints/gps-20261003/cold-start.{json,log}`.

The RAM-only implementation passed Clippy on both boards, 20 JS behavior tests,
14 USB/protocol Python tests and UI build consistency. A test verifies historical
coordinates are labeled, UTC stays unknown and copying as a current fix is disabled.
No GNSS aiding commands or host-time injection were added: the no-fix observation
with zero reported satellites does not demonstrate a need for startup aiding.

Installed and digest-verified RAM-only application `26.10.2-03151818` on
`A222C62566851775`. Post-install live GGA reception passed, with no UART/parser
errors. No prior position or UTC was restored at startup. Physical retention
across loss of a previously acquired fix remains unverified in this run because
the receiver did not reacquire satellites; that display path passed the JS test.

## GSV/GSA follow-up at the unchanged window position

The user confirmed the antenna remained at the same window position throughout.
Installed/digest-verified diagnostic application `26.10.2-03152718`.
A 30-second read-only capture (15 samples) showed live GPS fixes and synchronized
UTC throughout. The first observation already had a fix, so the actual
reacquisition time and total cold-start TTFF remain unknown.
Final satellites used: 7. GSV reports 13 GPS and
2 BeiDou satellites in view. Some CN0 fields are blank; nonempty values range
from 10 to 34 dB-Hz in the final captured cycle. GSA reports a 3D fix. Parser
and UART errors remained zero. Location data is kept in local Git-ignored evidence.

Earlier wording that GGA 0 proved no visible satellites was incorrect: that
field only counts satellites used in the fix. Current data confirms reception
and positioning at the same location. Weak/absent signals and high reported
HDOP (22.1 in an inspected packet) could contribute to acquisition delays;
a root cause was not proved. No GNSS configuration changes were made.
Observed outputs confirm GPS/BeiDou operation, but stored configuration registers
and backup-rail voltage were not queried or measured.

Added `get_gps_sentence` (15, one index byte 0–23), exposing a bounded RAM history
of checksum-valid NMEA packets with their ages. No flash writes/background logs.
Evidence: `dist/checkpoints/gps-20261003/gsv.{json,log}`, `gsv-summary.json`.
Validation: Clippy both boards, 55 Rust library tests, 20 JS behavior tests,
strict command argument checks and generated protocol/UI consistency.

## Startup measurement preparation

Installed and digest-verified application `26.10.2-03204522` on the selected XIAO.
Added RAM first-event timestamps via command 16 and a serial-selected incremental
JSONL recorder with a hardware lock, bounded waits, reconnect handling and fsync.
Validation: 56 Rust tests, both board Clippy checks, 94 Python tests with 8 HIL
skips before one additional passing recorder reconnect test, and 20 JS checks.
Readiness capture (`measurement-ready.jsonl`, local ignored evidence) showed
fix in all three samples, 6 satellites used, GPS UTC, no invalid NMEA or UART
errors. First GGA fix was observed at firmware monotonic 905 ms, but the GNSS
module was not confirmed power-cycled; this is NOT a cold-start TTFF result.
Three user-confirmed full power cycles and RF loss/restore checks remain pending.

### Cold run 1 — same window position

User confirmed removal of all main power, then restoration. Recorder observed
33.57 s between USB absence and application reconnection; this is an observation
bracket, not a precise physical off interval. In the first sample (firmware
uptime 1.285 s), fix was absent, UTC unknown and RAM last-known position absent.
First byte: 782 ms; valid NMEA: 814 ms; GSV view and positive CN0: 2955 ms.
First valid GGA position: **818359 ms (13 min 38.359 s)**; first active RMC UTC:
**818822 ms**. These are firmware monotonic milestones, not exact physical
power-on TTFF. At approximately 203 s, GSV reported 3 GPS + 1 BeiDou with
CN0 30–35 dB-Hz and GSA no fix; at approximately 510 s, 3 GPS + 3 BeiDou
with CN0 22–33 dB-Hz, still no fix.

Completed 900 s after application connection: 446 samples, final 6 satellites
used, zero invalid NMEA/UART errors. All 41 samples from first observed fix
through the end retained fix. Local evidence: `run-1.jsonl`,
`run-1-operator.jsonl`, `run-1-summary.json`. Runs 2/3 and loss-of-signal checks
remain pending. No receiver configuration changes were made.

### Cold run 2 — same window position

User confirmed full main-power removal and restoration. Observed USB absence
bracket: 46.00 s. Initial sample at firmware uptime 1.257 s had no fix,
unknown UTC and no RAM last-known coordinates. First byte: 782 ms; valid NMEA:
814 ms; GSV view and positive CN0: 1955 ms. At approximately 139 s, GSV showed
4 GPS + 1 BeiDou with CN0 24–39 dB-Hz, without a fix.

First GGA position: **268614 ms (4 min 28.614 s)**; first active RMC UTC:
**268999 ms**, both firmware monotonic rather than exact power-on TTFF.
Completed 900 s after application connection: 446 samples, 313 with fix,
zero loss samples after the first observed fix, final 6 satellites used,
zero invalid NMEA and UART errors. Local evidence: `run-2.jsonl`,
`run-2-operator.jsonl`, `run-2-summary.json`. Receiver settings unchanged.
Run 3 and signal-loss checks remain pending. The observed acquisition spread
between the first two runs does not establish its cause.

### Cold run 3 — same window position

User confirmed full main-power removal and restoration; observed USB absence
bracket 37.38 s. Initial sample at firmware uptime 1.427 s had no fix, unknown
UTC and no RAM last-known coordinates. First byte 782 ms; valid NMEA 814 ms;
GSV view and positive CN0 2955 ms. At approximately 125 s, GSV reported
6 GPS satellites with CN0 21–40 dB-Hz, zero BeiDou in that captured cycle.
First GGA position **131452 ms (2 min 11.452 s)**; first active RMC UTC
**131812 ms**, relative to firmware monotonic origin, not exact power-on TTFF.
Completed 900 s after application connection: 444 samples, 379 with fix,
no loss samples after first observed fix, final 8 satellites used, zero invalid
NMEA/UART errors. Local evidence: `run-3.jsonl`, `run-3-operator.jsonl`,
`run-3-summary.json`.

All three user-confirmed main-power cycles acquired position within the 15-minute
window. Observed first-fix milestones: 818.359, 268.614, 131.452 s; median
268.614 s. RAM position/UTC clearing was observed in every initial sample.
No receiver restart/configuration/aiding commands were sent. These measurements
demonstrate variable acquisition at this window, not a proven settings or RF
fault; they do not yet justify a speculative configuration or AGNSS change.
Physical RF-loss/holdover and restore checks remain pending.

### RF shielding attempt — loss not induced

After run 3, kept power on and started a separate recorder. User fully covered
the board/antenna with a steel cooking pot, retaining the window placement.
Support is plastered hollow ceramic on the fourth floor. User has no material
for an insulated metal bottom and cannot add it without moving the board;
no further shielding modification was performed.

The entire recording (including baseline and user coordination) contained
279 samples over 563.13 s, no loss of fix, no holdover samples, final 9 satellites
used and zero invalid NMEA/UART errors. This duration is NOT a precisely timed
covered interval. GSA during shielding continued reporting 3D fix. Recorder
was stopped deliberately with SIGINT; all incremental evidence was preserved
and hardware lock released. Files: `signal-loss.jsonl`,
`signal-loss-operator.jsonl`, `signal-loss-summary.json` (local ignored evidence).

Outcome: **inconclusive RF-loss test; shielding did not induce loss**. Physical
UTC holdover, last-known position display/copy disabling during actual RF loss,
and correction after reception restoration remain unverified. Existing logic
and automated tests are not substitutes for those physical checks. No module
restart or power cycle was substituted for RF loss. No GPS recorder remains
running. The three cold-start measurements and RAM clearing checks are complete.

### Completed RF loss / holdover / restoration check

User subsequently authorized temporary movement deeper inside the room and
moved the board with the laptop, preserving USB power/connection. This is an
indoor RF-loss check, separate from the three cold starts at the window.
Fix loss was observed with zero satellites used, while valid NMEA continued.
A measured holdover segment of 26 samples spanned 50.542 s: board UTC advanced
50542 ms, matching the monotonic advance of 50542 ms. RAM last-known coordinates
stayed unchanged and their age increased. No UART/NMEA or transport errors.
This establishes clock continuity, not absolute UTC accuracy without PPS.

Agent inspected the user's connected local USB page during actual loss:
“Last known location” age advanced from 128 to 135 s, displayed board clock
advanced from 04.10.2026 01:00:31 to 01:00:38, geographic zone was explicitly
estimated from the last location, and Copy coordinates was disabled.
The page showed “Board clock running” with an increasing correction age.
Agent disconnected the page to release USB, without removing physical power.

A second recorder began in holdover before the user returned the board to the
original window. Current GGA position and active GPS UTC updates returned;
first restored fix appeared 38.437 s into this recorder. This is NOT an exact
physical-return-to-reacquisition delay, because return timing was user-controlled.
The restored fix remained present for all subsequent samples over 66.815 s;
final 7 satellites used, time source GPS, correction age 86 ms. Monotonic uptime
continued across loss, UI inspection and restoration, with original startup
milestones intact; no reset occurred. Both recorders stopped deliberately and
hardware lock was released. Local ignored evidence: `room-loss.jsonl`,
`room-restore.jsonl`, `room-loss-operator.jsonl`, `room-loss-summary.json`.

**Approved measurement plan completed:** all three full-main-power starts,
RAM clearing, physical RF loss, holdover, historical UI/copy disabling and
restored GPS position/time correction verified. Cold-start first-fix spread
131.452–818.359 s at this window remains unexplained. No startup-setting or
AGNSS changes are justified solely by these observations; any such work needs
a separate evidence-based investigation. No GPS recorders remain running.
