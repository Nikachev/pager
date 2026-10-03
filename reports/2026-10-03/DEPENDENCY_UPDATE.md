# Dependency update — 2026-10-03

Versions were checked against the crates.io and PyPI APIs, and the upstream Git
repositories. All application/tool/bootloader direct registry dependencies now
request the latest stable release available on this date. All five committed
Cargo lockfiles were refreshed to the latest versions permitted by their dependency
graphs. Rust's toolchain is unchanged; prereleases are not selected as upgrades.

| Dependency | Before | After |
| --- | --- | --- |
| trouble-host | 0.7.0 | 0.8.0, release tag `trouble-host-v0.8.0`, commit `42e3e04de00db3951b126741e1f3602bfdf5df47` |
| trouble-host-macros | 0.5.0 | 0.6.0, same release source |
| bt-hci | 0.9.0 | 0.10.1 |
| nrf-sdc / nrf-mpsl Git source | `f54b6389` | `e44e619d15eba9145453e3b2d7b07121556871ab`, upstream release commit |
| nrf-mpsl | 0.3.0 | 0.4.0 |
| nrf-sdc-sys / nrf-mpsl-sys | 0.2.1 | 0.3.0 |
| usbd-storage | 0.2.0 | 3.0.0, published source commit `f2015e7554cec0a4f1c5433b1730f01bf96cbdf8` |
| cortex-m / cortex-m-rt | 0.7.8 / 0.7.6 | 0.7.9 / 0.7.7 |
| ed25519-compact | 2.3.1 (xtask requirement 2.2.0) | 2.6.0 |
| embedded-storage / embedded-storage-async | 0.3.1 / 0.4.1 | 0.3.2 / 0.4.2 |
| sha2 in bootloader-updater | 0.10.9 | 0.11.0, matching bootloader-core |
| futures | 0.3.33 | 0.3.34 |
| ruff | 0.16.9 | 0.16.10 |

Embassy crates, heapless, static_cell, defmt, panic-probe, usb-device, nrf-usbd,
ed25519-dalek, pkcs8, sha2 elsewhere, and the other Python requirements already
resolve to the current stable release. Minimum versions for serde_json, futures
and other direct dependencies were made explicit in the project manifests.

“Latest” for transitive dependencies means the latest version compatible with
the upstream library's requirements. Trouble 0.8.0 still requires aes 0.8,
cmac 0.7, p256 0.13 and rand 0.8/ rand_core 0.6; its macro crate requires syn 2,
darling 0.20 and convert_case 0.8. Newer incompatible major versions exist. These
upstream constraints are preserved; blindly replacing them would introduce a
separate cryptography/macro migration. The current Trouble main branch has a
new crypto backend, but that is not part of the published 0.8.0 release.

## Trouble patch decisions

| Patch | Decision | Functional reason |
| --- | --- | --- |
| Security mutex feature gates in `host.rs` | Removed | 0.8.0 already gates `NoopRawMutex` on scan or security, and `Mutex` on security. Peripheral-only security compiles without central. |
| `Stack::set_runtime_local_address` | Retained | SMP must use the selected profile's on-air random-static address. The upstream builder sets the initial address, but there is no runtime setter for changing the SMP identity without changing the controller-global address. |
| `Peripheral::advertise_ext_from_handle` | Retained | Profiles map permanently to controller handles 0–2. Upstream assigns handles starting at zero on every call. Reusing one handle for different identities loses that profile/set mapping. |
| `Peripheral::advertise_ext_prepared` | Retained | Reconnect re-enables an existing set without rewriting its identity or parameters; advertising and scan-response data must still refresh after rename or bond clear. Upstream `advertise_ext` rewrites parameters/address; `update_adv_data_ext` only changes data, without starting a prepared set. |

The retained methods follow the new `extended-advertising` feature gate. HCI 0.10
uses extended duration values for extended advertising intervals, while enable
command timeouts still use the ordinary duration type. Trouble's central role
remains disabled. nrf-sdc's separate central+peripheral gate on
`support_ext_adv()` remains unchanged upstream, so its workaround is retained.
After formatting normalization, only Trouble `lib.rs` and `peripheral.rs` carry
local semantic changes. The macros have no semantic patch.

## usbd-storage migration

The new upstream BOT state machine and buffer implementation replace the old
transport and its short-transfer/bounded-buffer workarounds. Pager retains its
strict CDB decoder (including big-endian READ/WRITE, truncated command rejection
and READ CAPACITY(16) service-action checking), START STOP/SYNCHRONIZE CACHE,
64-bit adapter fields, simple interface descriptor and local CBW generations.

A small command adapter preserves the bootloader's callback API while driving
BOT 3.0. It counts bytes accepted by each read/write and supplies that count to
upstream's new explicit `set_status(status, bytes_processed)` API. IN writes
are bounded by the remaining CBW length even when several writes occur before
endpoint polling. Generations advance per CBW, independently of host tag reuse. The OUT state
also remains readable until Pager consumes the final buffered packet: repeated
USB polls must not close the data phase before the callback runs. Its private
OUT handler checks its dispatch state with a debug assertion: `Transport::poll`
only invokes it for that state. Public `set_status` retains its runtime state
assertion. This removes 48 bytes while preserving the 48,128-byte boot budget.
Upstream BOT unit tests remain; registry integration tests target the original
command API and are excluded from the Pager subset. UFI remains excluded.

## Validation

The final `make quality` completed successfully: 164 Rust tests, 92 Python
tests and 17 UI tests, all lint/Clippy/vendor checks, and the complete build and
package matrix. Nice application: 281,316 bytes; XIAO application: 281,364 bytes
(budget 974,848). Both bootloaders: 48,088 bytes (budget 48,128). Static RAM
checks passed for both boards. The bootloader has only 40 bytes left within
the enforced budget, which already reserves 1 KiB of its flash partition.

Host tests cover SMP identity changes without any controller command, strict
CDB decoding, BOT's thirteen transfer cases, partial OUT read residue, bounded
multiple IN writes, reused host tags with new CBW generations, and a 1024-byte OUT transfer with
repeated USB polling before final-buffer consumption. The full
`make quality` gate covers host/Python/UI tests, formatting, lint, Clippy, vendor
hash inventory, both boards' firmware/bootloader/updater builds, the XIAO
installer, signing/package verification, image size, layout and protocol checks.

Hardware runs are documented for [nice!nano](DEPENDENCY_HIL_NICE.md) and
[XIAO](DEPENDENCY_HIL_XIAO.md). Both completed the planned USB/UF2, bonded
switching, rendered HID, fresh/cached pairing and cold-start checks. Nice!nano
also passed rendered switching across three independent hosts. Remaining
observations are one nice!nano Mac reconnect timeout and an initial failed XIAO
cached-pair attempt; successful retries do not erase those failures.
The [earlier validation](../2026-10-02/FINAL_VALIDATION.md) qualifies only its recorded older images.
