# Persistent storage v6

Pager deliberately does not migrate pre-v6 development data. The first v6 boot
starts from Bluetooth Off and empty slots. Thereafter ordinary application UF2
updates preserve storage.

Two 4 KiB pages form an append-only copy-on-write journal. Each aligned 272-byte
record contains a 40-byte header, three independent 76-byte slot records, CRC-32
and a final commit word. The commit word is programmed last. Boot selects the
newest valid wrapping sequence; an interrupted erase/write/commit therefore
leaves the previous page record valid.

The header stores `PGS6`, schema 6, active slot/none, Bluetooth enabled, sequence
and the common 24-byte UTF-8 name. Each occupied slot stores address kind/address,
optional IRK, LTK, security level, bonded flag and a 32-byte UTF-8 alias. Empty
slots contain no security data. Semantic CCCD flags are persisted because
macOS treats HID subscriptions as bonded peripheral state and may not write
them again after a firmware reboot. A trusted bond with both HID input bits
missing is healed once by enabling Report and Boot Keyboard notifications.

The storage task scans once at startup, caches page/slot/sequence, serializes
save request IDs and advances the cursor only after the commit succeeds. On an
I/O error it reloads the last durable record and rolls runtime/UI state back.
Factory reset appends the fresh-device state: all bonds/aliases/settings cleared,
name `Pager`, Bluetooth Off and no active slot.
