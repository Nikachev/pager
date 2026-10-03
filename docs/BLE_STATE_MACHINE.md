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

A peer identity occupies at most one slot. Pairing it in another slot removes
its old bond, CCCD flags and alias. Use a third host to test slot 3 while retaining
the first two pairs.

## Diagnosing connection readiness

A host's “paired” label does not prove Pager committed a bond or reached HID
readiness. Check the occupied slot, connected profile, encryption and keyboard
notification subscription before testing rendered input. Reconnect timing varies;
preserve state and BLE logs on timeout before retrying. Intermittent Mac readiness
timeouts and incomplete Android re-pairing have been observed; their causes are
not yet established. A successful retry does not establish that a failure is fixed.
