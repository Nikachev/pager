# Autonomous control pages

Open `webusb_client.html` in a WebUSB-capable browser such as Chrome and select
Pager with **Connect USB**. Keep other WebUSB pages and Python USB tools
disconnected while this page owns the device. **Disconnect USB** releases the
interface and closes the device; reconnecting starts a new reader and request
queue. Unsupported browsers show an explanation with the connect button disabled.
`ble_client.html` explains pairing through macOS or Android Bluetooth settings.
Neither page uses network resources or a frontend framework.

Turning Bluetooth off or on clears the active slot; choose a slot after turning
it on. Saved bonds and aliases remain. An unsuccessful pairing window expires after
120 seconds and turns the radio off; start a new pairing attempt explicitly. The USB page manages the radio, three slots, device and slot names,
keyboard text, diagnostics, bootloader reboot and factory reset. A device name is
limited to 24 UTF-8 bytes, a slot alias to 32, and keyboard text to 256. These are
byte limits: some characters need more than one byte. Firmware can reject keyboard
characters that have no supported HID mapping. A failed send keeps the text field.

One operation runs at a time. Event bursts request a coalesced state snapshot;
an event overflow or revision gap also refreshes the snapshot. The current
connection owns every reader, pending request, timer and write. Timeout, transport
failure and disconnect reject outstanding requests and release USB resources;
a previous reader cannot update a later connection. Logs retain at most 200 rows,
with at most 4096 characters per message. Command payloads and typed text are not
written to the browser log. Device diagnostic output is already redacted by firmware.

Native dialogs have accessible names. Confirmation opens on Cancel. Escape
cancels; closing restores focus. Slot controls have explicit labels and preserve
keyboard focus across state snapshots. Manual keyboard checks are required in addition to mocked DOM tests. VoiceOver
qualification is outside this cycle by user decision.

For keyboard testing, prepare an empty input on the receiving host before Send.
Using the USB page on the Mac while sending to Android permits the Android target
field to retain focus. Send has no delay for changing the active Mac window;
do not use it when the Mac has no safe focused input. Diagnostic read and rename
operations send no HID text. Factory reset permanently removes bonds, names and
settings, so pair both hosts again afterwards. Reboot enters the bootloader; restore
an ordinary application with a validated board/serial-bound UF2 package.

## Sources and checks

Edit the templates and CSS in `web/` and the `usb_session.js` / `usb_app.js` modules.
The shared spec and codec are generated/checked against `protocol.json`.

```sh
.venv/bin/python tools/generate_protocol.py
cargo run --locked --target aarch64-apple-darwin -p xtask -- build-ui
cargo run --locked --target aarch64-apple-darwin -p xtask -- check-ui
```

Root HTML and `dist/ui/` copies embed all their source. `check-ui` verifies freshness,
autonomy and `node --test tests/ui_behavior.cjs`; golden wire vectors are checked
separately. Tests exercise real JS modules with deterministic USB and DOM mocks,
including asynchronous open cancellation, claim failure, partial frames, request
wrap/collision, pending cleanup, stale readers, timeout, malformed state/error/ACK,
event burst/overflow, UTF-8 limits, busy controls, bounded logs and dialogs. Mocks
cannot establish actual browser focus, VoiceOver output or rendered BLE text.

## Hardware acceptance

Use one known XIAO with Mac in slot 1, Android in slot 2 and slot 3 empty. Record the
installed version/identity and test results in a qualification report; see
[final validation](FINAL_VALIDATION.md) for the completed hardware matrix.

1. Connect the USB page; verify slot/radio state and HID-ready Send availability.
2. Change the device name and slot alias, then restore their original values.
3. Turn Bluetooth off/on; switch slot 2 then slot 1; verify snapshots and reconnect.
4. Read diagnostics; verify operation feedback and bounded display.
5. Use Tab and Escape for dialogs; verify focus return and control labels.
6. On Android, focus a safe empty input, send a known ASCII string through the UI
   and compare the actual rendered text exactly.
7. Reboot to bootloader through the UI, restore a trusted ordinary application,
   reconnect and verify the same chip/version and durable state.
8. Announce and confirm factory reset, verify the fresh state, pair Mac/Android
   again and repeat slot switching and rendered input checks.

Keep hardware acceptance separate from host-test success. UI automation failures
are limitations of the test route until reproduced in the actual page.
