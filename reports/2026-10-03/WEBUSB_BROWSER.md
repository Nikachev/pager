# WebUSB browser checks — 2026-10-03

Route: ordinary Chrome, `http://localhost:8000/webusb_client.html`, generated
standalone page served from `dist/ui/`. Connected fixture: XIAO, `PagerXiao`,
Mac in slot 1, Android in slot 2, slot 3 empty.

## Passed checks

- Page accessible to browser automation through localhost. Chrome device picker
  required user selection; this is separate from page DOM access.
- WebUSB connected, displayed saved slots and connection/readiness state.
- Device name and slot-1 alias changed, displayed, then restored.
- Bluetooth Off/On preserved bonds; On required selecting a slot. Mac and Android
  switching refreshed state; Send stayed disabled until HID readiness.
- Read device log completed. Typed payload was absent from page logs.
- Rename Escape restored focus. Confirmation opened on Cancel; Tab reached
  Continue, Escape canceled and restored the invoking slot-control focus.
- Free-slot, factory-reset and bootloader confirmations were canceled.
- Send to prepared Android field completed; user confirmed `Pager WEBUSB OK `.
- Disconnect cleared controls; a second user device selection reconnected and
  restored the correct slot state. No warning/error console entries were captured.
- Generated-page check and all 17 UI behavior tests passed.

Final public fixture: `PagerXiao`, original slot aliases and two bonds, active
Android slot 2, slot 3 empty. Browser remains connected, so Python tools must wait
until Disconnect USB is clicked.

Evidence: local `dist/checkpoints/dependencies-20261003/xiao-nrf52840/`:
`webusb-browser-dom.txt`, `webusb-browser.jpg`, `webusb-check-ui.log`.
Destructive reset/reboot execution, screen-reader output, USB OTG and the BLE
instruction page were not qualified in this browser run. UF2 hardware recovery
was tested separately in the XIAO dependency report.

## Resulting changes

Added `make serve-ui`: regenerate standalone pages, serve only `dist/ui/` on
127.0.0.1:8000 and print the control URL. README and the web UI guide now describe
this route, browser device permission and server shutdown. No UI defect requiring
an application-code change was reproduced. Localhost solves this tested access
route; it does not prove every earlier file-URL failure had the same cause.
