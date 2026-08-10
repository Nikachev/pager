# Pager advertising-set identity patch

Upstream source: `embassy-rs/trouble` commit
`088e09c451177d5db50cf3e58c68d05512265ba0`.

Pager adds only `Stack::set_runtime_local_address()`. Extended advertising can
assign a random-static address to an advertising set without changing the
controller-global address. SMP must nevertheless use that on-air address in its
pairing calculations, so the method updates Trouble's host address and Security
Manager state without issuing any HCI command.

Pager also adds `Peripheral::advertise_ext_from_handle()`. Pager permanently
maps slots 1–3 to advertising handles 0–2, avoiding changes to the identity
address of an existing nRF-SDC advertising set.

`Peripheral::advertise_ext_prepared()` re-enables such a set without rewriting
its parameters or identity after it has previously accepted a connection.

Pager also fixes the feature gate on the security command mutex imports in
`host.rs`; security in a peripheral-only build uses that mutex and must not
require Trouble's unrelated `central` feature.
