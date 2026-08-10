# BLE state machine

Persisted global state is Bluetooth enabled, active slot, common name, three
bonds and three aliases. Runtime-only state adds connected slot, pairing,
advertising phase and HID readiness.

- `BluetoothOff`: no active slot; no advertising. Enabling moves to `Idle`.
- `Idle`: Bluetooth enabled with no slot. Selecting an empty slot starts
  `Pairing`; selecting an occupied slot starts `Advertising`.
- `Pairing`: fast discoverable advertising for 120 seconds. A bond is committed
  only to the selected transaction slot. Timeout/cancel returns persistently to
  `BluetoothOff`.
- `Advertising`: the selected identity and only its bond are active. Reconnect
  uses 10 seconds fast advertising, then the low-power interval indefinitely.
- `Connecting`: link exists while security/GATT are established.
- `Connected`: peer identity matches the selected bond. `HidReady` additionally
  requires encryption and a keyboard input CCCD subscription.
- `Disconnecting`: slot, radio, name or factory-reset command is being applied.

Advertising can stop and restart without reconstructing the GATT server. One
server instance belongs to the firmware uptime and is shared by every state
transition and replaceable radio/controller session.

Selecting the already active occupied slot is a no-op. Clearing and pairing are
separate operations. Clearing removes only that slot; selecting it afterward
starts pairing. A failed occupied-slot reconnect leaves it active. Slot aliases
default to the peer MAC and can be renamed independently. Advertising uses the
common name plus an automatic ` 1`, ` 2`, or ` 3` suffix.
