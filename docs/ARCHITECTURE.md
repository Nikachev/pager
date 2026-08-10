# Architecture

The nRF52840 starts in a 48 KiB synchronous signed-UF2 bootloader. A valid
single-slot image transfers control to the Embassy application at `0xC100`.
`layout.json` generates both linker layouts and Rust/xtask constants; the final
two flash pages are never included in an application UF2.

The application keeps USB alive for its entire runtime. Independent Embassy
tasks drive USB, WebUSB framing, CDC diagnostics, persistent storage, watchdog
and BLE. BLE commands are serialized through one bounded queue and only report
success after their side effect and durable write complete. A versioned internal
state event channel fans out to WebUSB and can later feed an on-device UI.

The BLE host exposes only GAP/GATT, HID Keyboard and Battery Service. A supervisor
owns advertising, connection, security and slot transitions. Every slot uses a
derived random-static address assigned to its extended advertising set; only
the selected slot's bond is loaded and the advertised name carries its suffix.
The controller remains alive across slot changes, while Bluetooth Off destroys
the radio session without affecting USB.
The generated GATT server is constructed once per firmware uptime and reused
across advertising phases, slot transitions and radio Off/On sessions. Its
characteristic values use one-shot `StaticCell` storage, so reconstructing it
after stopping the controller is invalid.

Pager carries a small Trouble patch: a slot-specific advertising address is
mirrored into the host Security Manager so SMP uses the actual on-air identity,
and fixed advertising handles can be re-enabled without rewriting identity
parameters.
Trouble is built without its central feature. `nrf-sdc` still gates the
`support_ext_adv()` builder API on `central + peripheral`, so only its compile-time
feature remains as an upstream workaround; Pager never configures a central role.
