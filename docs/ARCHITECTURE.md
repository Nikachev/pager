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
parameters. Advertising and scan-response data are refreshed before each
prepared-set enable, so pairing flags and renamed local names cannot stay stale.
Trouble is built without its central feature. `nrf-sdc` still gates the
`support_ext_adv()` builder API on `central + peripheral`, so only its compile-time
feature remains as an upstream workaround; Pager never configures a central role.

Runtime modules separate board setup, the BLE session supervisor, persistence
and diagnostics. The reducer/effects layer applies control and session events;
request IDs and terminal result history prevent late or evicted completions from
being reported as a new success. HID scheduling and semantic CCCD helpers are
shared with host tests. Watchdog heartbeats supervise USB, BLE and storage.

`protocol.json` generates the Rust/Python/JS contract. The bounded frame stream
handles split and combined USB packets; one reader dispatches responses and
revisioned events. Event overflow is explicit and clients recover from a full
snapshot. The Python transport and browser session each own their request map,
resources and cleanup. Browser generations reject stale readers after reconnect.

The parser appends only the available part of a USB packet, drains complete
frames, and then appends the remaining bytes. This preserves maximum-frame plus
next-header handling without parsing after each byte. An owned frame initializes
only its actual payload in a bounded heapless vector. `next_frame_into` fills
caller-owned storage; the asynchronous USB task retains that buffer across reply
awaits, avoiding whole-capacity vector copies through return values. Partial and
invalid frames leave the last valid output intact.

Both application and bootloader use the pinned `vendor/embassy-nrf` USB patch.
OUT DMA completion already re-arms the hardware endpoint; the driver must not
write `SIZE.EPOUT` again on that path, because it can discard an unread packet.
Initial endpoint enable still writes SIZE. Provenance, licenses and update checks
are recorded in `vendor/embassy-nrf/PAGER_PATCH.md` and `vendor/PATCHES.json`.

`bootloader-core` provides the no_std manifest/UF2/layout/vector codec, flash
transaction engine and sector cursor used by the bootloader, updater and xtask.
The engine commits a validated transfer only after flash readback and digest,
signature and vector validation. Hardware adapters perform NVMC operations;
mock adapters exercise interruption and retry behavior without a board.
