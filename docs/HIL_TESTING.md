# Manual HIL fixture

HIL runs locally on macOS and is never part of CI at this stage. Preparation is
manual: connect Pager over USB, pair slot 1 with the Mac running pytest, and pair
slot 2 with another host. Preserve both bonds. Once prepared, the automated run
must not ask for confirmation, clear slots, pair devices, or require interaction.
Close or disconnect the WebUSB browser client before starting HIL: libusb needs
exclusive access to the vendor interface. An SWD probe may remain connected;
serial discovery filters it by Pager's application VID/PID.

```sh
make test-hil
# destructive bootloader/flash coverage, when explicitly intended:
make test-all
```

Automated HIL verifies USB discovery/framing, state events, persistence, slot 2
activation without requiring the uncontrolled host to wake, and reconnection/HID
readiness on the local Mac in slot 1. It does not verify rendered keystrokes or
automatic reconnection on slot 2.
The same WebUSB session also turns Bluetooth fully off, verifies that no slot is
active while both bonds remain stored, turns it on, and reconnects slot 1.
Manually confirm after the run that switching slot 1 → slot 2 → slot 1 reconnects both
prepared hosts and that text is typed on each, while WebUSB stays connected.

WebUSB over Android USB OTG is currently outside the test matrix. The supported
Android acceptance is Bluetooth pairing/reconnection on the latest Android and
Chrome, with the advertised Pager name and slot suffix.
