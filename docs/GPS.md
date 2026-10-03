# L76K GNSS on XIAO

The `board-xiao-nrf52840` application starts the Seeed Studio L76K receiver
at its factory UART setting, 9600 baud, 8N1. XIAO D7/P1.12 receives module
TX; D6/P1.11 connects to module RX. Mount the expansion board in the correct
orientation, with 3V3 and GND aligned, and attach its active U.FL antenna.
See the [Seeed L76K guide](https://wiki.seeedstudio.com/get_start_l76k_gnss/)
and [XIAO pin mapping](https://wiki.seeedstudio.com/XIAO-BLE-Sense-Pin-Multiplexing/).

The firmware reserves UARTE0, TIMER2, PPI channels 0/1 and PPI group 0 for a
1024-byte buffered receiver. GPS runs independently of USB and Bluetooth.
The nice!nano build does not initialize GPS peripherals.

WebUSB command `get_gps` (14), with no arguments, returns semicolon-separated
metadata. It extends protocol v5 without changing existing commands/state.

- `supported`: 1 on XIAO, 0 on nice!nano.
- `connected`: a checksum-valid NMEA sentence arrived within the last 5 seconds.
- `fix`: a checksum-valid GGA with nonzero fix quality arrived within 5 seconds.
- `bytes`, `valid`, `invalid`, `uart_errors`: cumulative receive diagnostics.
- `quality`, `satellites`: values from the latest GGA; inspect `gga_age_ms` before
  treating them as current.
- `nmea_age_ms`, `gga_age_ms`: time since last corresponding sentence;
  18446744073709551615 means none received.
- `gga`: latest validated GGA, including UTC, coordinates and altitude when fixed;
  optional when the response would exceed 512 bytes.

GPS coordinates are available only through this explicit diagnostic request;
they are not persisted or sent in background logs. Normal NMEA reception proves
the module-to-XIAO data path; it does not prove the antenna receives satellites
or the unused XIAO-to-module command path. This implementation listens to the
factory stream and does not change the module's configuration.

Build and install on one selected board:

```sh
PAGER_BOARD=xiao-nrf52840 make build
.venv/bin/python tools/flash_uf2.py --file dist/xiao-nrf52840/dev/app/pager.uf2 \
  --board xiao-nrf52840 --serial YOUR_CHIP_SERIAL
```

Check live GGA reception without new UART/checksum errors:

```sh
.venv/bin/python tools/test_gps.py --serial YOUR_CHIP_SERIAL --timeout 30
```

Check satellite positioning with the antenna outdoors or near a window:

```sh
.venv/bin/python tools/test_gps.py --serial YOUR_CHIP_SERIAL \
  --require-fix --timeout 180 --output dist/gps-check.json
```

No bytes suggests power, mounting, UART wiring or changed module baud settings.
Bytes with invalid checksums suggest noise or incorrect UART settings. A clean
stream with `quality=0` shows a working serial connection without positioning;
check the antenna connection and sky visibility. Disconnecting the receiver
expires `connected` and `fix` within 5 seconds, even if old GGA data remains.

## Coordinates and board clock

`get_gps` also returns signed `latitude_e6`/`longitude_e6` in millionths of a
degree when `fix=1`. Unknown/stale coordinates use the sentinel
9223372036854775807; clients must inspect `fix` before displaying coordinates.

A checksum-valid RMC with active status and a valid calendar date corrects the
shared board UTC clock. `pager::clock::Clock` contains the host-testable clock
logic; firmware tasks use `crate::clock::now_utc_ms() -> Option<u64>` and
`crate::clock::sync_age_ms()`. Before the first valid correction, UTC is `None`.
After correction, the clock advances using Embassy's monotonic RTC clock even
without GPS reception. It is not persisted and resets to unknown on reboot.
Corrections may step UTC forward or backward; use `embassy_time::Instant` for
durations and deadlines. RMC years use the NMEA 1980–2079 window; leap-second
values of 60 are rejected. Accuracy is limited by NMEA serial delivery latency;
this is not a PPS-disciplined precision clock.

- `utc_ms`: current board UTC, Unix milliseconds; 0 if unsynchronized.
- `time_source`: `unsynced`, `gps` (correction within 5 seconds), or `holdover`.
- `time_age_ms`: monotonic time since the last correction, or u64::MAX if none.

The standalone USB page polls GPS once a second while idle. It displays live
coordinates, board time, satellite count and synchronization/holdover status.
Between samples it advances the board UTC sample using `performance.now()`;
it never substitutes the computer's wall clock. A USB sample older than five
seconds clears current coordinates and hides the displayed board time.

The page resolves an estimated IANA time zone locally with the bundled
@photostructure/tz-lookup 11.7.0. It applies the browser's Intl time-zone rules,
including daylight saving, to the UTC instant from the board. No coordinates
are sent to a service. Geographic lookup is approximate near zone boundaries;
the page labels this explicitly. Until the first GPS position it uses UTC.
After a fix is lost it retains and labels the zone from the last location.
The UTC source on the board is independent of browser/time-zone presentation.
See `web/vendor/NOTICE.md` for provenance and licenses.

## No backup supply / no GPS flash writes

This installation has no L76K backup supply. Full removal of main power must
be treated as a cold start; software resets of XIAO do not prove module power
loss. Firmware does not persist GPS coordinates, dates, clock anchors, orbit
data or time zones. Only the existing rarely changed device/BLE settings use
flash. No storage schema changes are needed.

While the firmware stays powered, the last valid position and its timestamp
are retained exclusively in RAM. `get_gps` optionally returns
`last_latitude_e6`, `last_longitude_e6`, `last_fix_utc_ms` (0 if UTC was unknown)
and `last_fix_age_ms` (monotonic age). These do not set `fix=1` or synchronize
UTC. The page labels these coordinates as the last known location, shows age,
and disables copying them as a current location. All clear on reboot/power loss.
The page derives the last time zone locally from that last RAM position.

For a physical cold-start check, remove all main power from both boards, restore
power, then run `tools/test_gps.py --serial YOUR_CHIP_SERIAL --wait-device 60
--require-fix --timeout 180`. The wait option only waits for USB enumeration;
it does not switch board power. TTFF from actual power-on requires starting
observation at power-on; late host polling cannot establish an exact TTFF.

## Satellites in view diagnostics

GGA `satellites` is the count used in the fix, not the count in view. A count of
zero must not be described as proof of no visible satellites. GSV reports
satellites in view, with up to four per packet and multiple numbered packets
per constellation; nonempty CN0 fields report signal levels. GSA reports the
fix dimension and satellite IDs actually used.

Command `get_gps_sentence` (15) requires one index byte, 0–23. It returns a
packet from the bounded 24-entry RAM history, newest first, as
`age_ms=...;nmea=$...*XX`, or an empty response for a missing entry.
Only checksum-validated NMEA packets enter this history. Clients must inspect
age and multipart sequence numbers; the history is not an atomic complete
GSV snapshot. No sentences are saved to flash or background logs.

Capture explicitly on the selected XIAO:

```sh
.venv/bin/python tools/gps_diagnostics.py --serial YOUR_CHIP_SERIAL \
  --duration 30 --output dist/gps-diagnostics.json
```

This reads current outputs without changing receiver configuration or issuing
GNSS restart commands.

### Startup measurement recorder

`get_gps_startup` (16, no arguments) returns a separate bounded response with
`uptime_ms` and first byte / checksum-valid NMEA / GSV view / positive GSV CN0 /
valid GGA position / active RMC UTC timestamps. Empty values mean not observed.
All timestamps are RAM-only embassy monotonic milliseconds, not physical power-on
TTFF. Byte time is when the UART buffer is read; bootloader and buffering delay
remain outside the measurement. First events survive NMEA history rollover.

Arm `tools/gps_recorder.py --serial A222C62566851775 --cycle --duration 900
--output dist/checkpoints/gps-20261003/run-1.jsonl` before removing power.
The recorder requires USB absence before connection, uses the serial hardware
lock, and saves each JSONL event with flush/fsync. Initial wait defaults to 600 s;
observation is bounded to 900 s from first application connection, including
later transport interruptions. Output must be a new file. USB absence alone does
not prove removal of battery power: confirm full main-supply removal with the
user. Use separate files for three runs and `--cycle` omitted for signal-loss
observation without a power cycle. Preserve non-fix outcomes as timeouts; never
substitute an estimated acquisition time. Physical signal loss and UI behavior
still require coordinated hardware verification.
