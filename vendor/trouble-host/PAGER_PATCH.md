# Pager advertising-set identity patch

Upstream source: `embassy-rs/trouble` commit
`42e3e04de00db3951b126741e1f3602bfdf5df47`.

Pager adds only `Stack::set_runtime_local_address()`. Extended advertising can
assign a random-static address to an advertising set without changing the
controller-global address. SMP must nevertheless use that on-air address in its
pairing calculations, so the method updates Trouble's host address and Security
Manager state without issuing any HCI command.

Pager also adds `Peripheral::advertise_ext_from_handle()`. Pager permanently
maps slots 1–3 to advertising handles 0–2, avoiding changes to the identity
address of an existing nRF-SDC advertising set.

`Peripheral::advertise_ext_prepared()` re-enables such a set without rewriting
its parameters or identity after it has previously accepted a connection. It
refreshes advertising and scan-response data before Enable, so pairing
discoverability flags and renamed local names never remain from an older use.

Trouble 0.8.0 fixes the security command mutex feature gate upstream. The old
`host.rs` patch has been dropped; peripheral-only security builds use the upstream
imports unchanged. The remaining advertising methods follow the new upstream
`extended-advertising` feature gate. Extended interval parameters use the new
HCI extended duration type; enable timeouts retain the ordinary HCI duration.

The complete file/hash inventory is `vendor/PATCHES.json`, with maintenance and
regression requirements in `docs/VENDOR_PATCHES.md`. The host-macros copy has no
semantic delta from the same commit; the Rust copies also contain formatting and
comment whitespace changes. The advertising and host-address patches above are the remaining semantic
changes. A host test switches SMP identities and verifies that no HCI command
is issued. Dated dependency decisions and hardware validation are archived in `reports/`.
