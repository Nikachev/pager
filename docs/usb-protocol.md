# Pager USB protocol v5

Pager exposes a vendor WebUSB bulk interface alongside CDC-ACM diagnostics.
Integers are little-endian and frames may span USB transfers.

| Offset | Size | Field |
| --- | ---: | --- |
| 0 | 4 | `PGR1` |
| 4 | 1 | protocol version `5` |
| 5 | 1 | command `1`, response `2`, event `3`, error `5` |
| 6 | 4 | request ID; zero for events |
| 10 | 2 | payload length, at most 512 |
| 12 | 4 | IEEE CRC-32 of payload |
| 16 | n | payload |

Responses preserve the request ID. Events may be interleaved with responses, so
clients need one continuous reader and a request map rather than one read per
write. Error payload codes are bad request `1`, unsupported command `2`, busy or
timeout `3`, DFU failure `4`, HID not ready/subscribed `5`, unsupported character
`6`, connection lost `7`, and command queue full `8`.

## Commands

| Opcode | Command | Payload / result |
| ---: | --- | --- |
| 1 | `PING` | returns `PONG` |
| 2 | `GET_INFO` | protocol, boot model and build version |
| 3 | `GET_STATE` | schema below |
| 4 | `ACTIVATE_SLOT` | slot `0..2`; empty slot starts pairing |
| 5 | `CANCEL_PAIRING` | persistent Bluetooth Off |
| 6 | `SET_BLUETOOTH_ENABLED` | boolean; leaves no active slot |
| 7 | `CLEAR_SLOT` | slot; separate from subsequent pairing |
| 8 | `TYPE_TEXT` | up to 256 supported ASCII bytes; one job at a time |
| 9 | `REBOOT_TO_BOOTLOADER` | persists, replies, detaches USB, resets |
| 10 | `GET_LOGS` | bounded diagnostic history |
| 11 | `SET_DEVICE_NAME` | non-empty UTF-8, at most 24 bytes |
| 12 | `SET_SLOT_NAME` | occupied slot + non-empty UTF-8, at most 32 bytes |
| 13 | `FACTORY_RESET` | clears every bond/setting and returns Bluetooth Off |

Mutating success is sent only after durable persistence. `TYPE_TEXT` success is
sent after all press/release reports complete. Slot switching, pairing and device
renaming do not reset USB.

## State schema 5

The first ten bytes are: schema `5`, Bluetooth enabled, link state, active slot,
connected slot, pairing flag, three occupied flags, and `HidReady`. Missing slots
use `0xFF`. Link states are BluetoothOff `0`, Idle `1`, Advertising `2`, Pairing
`3`, Connecting `4`, Connected `5`, Disconnecting `6`.

They are followed by three length-prefixed aliases, three length-prefixed peer
MAC addresses, and the length-prefixed common Pager name. A newly paired alias is
its MAC address. Advertising names are `<name> 1`, `<name> 2`, `<name> 3`.

Event payload `1 + revision:u32` means state changed. Payload starting with
`0xFF` means event overflow. On connection, overflow, or a revision gap the client
must fetch a complete `GET_STATE` snapshot.

CDC remains a diagnostic/recovery interface; WebUSB v5 is the canonical control
protocol. Old opcodes and old frame versions are intentionally unsupported.
