# Local development builds

Use Python 3.14 or later: the tools use Python 3.14 exception syntax. Install
the dependencies pinned in `tests/requirements.txt`, the
`thumbv7em-none-eabihf` Rust target and
`cargo-binutils`/`llvm-tools-preview` for `rust-objcopy`. Node runs the shared
protocol vectors; the standalone HTML pages have no runtime network dependencies.

```sh
python3.14 -m venv .venv
.venv/bin/python -m pip install -r tests/requirements.txt
rustup target add thumbv7em-none-eabihf
rustup component add llvm-tools-preview
cargo install cargo-binutils --locked
make quality-fast
make quality
```

The fast gate runs host tests, formatting, Python lint, host Clippy, and the vendor
inventory. The full gate also builds and checks both board applications,
bootloaders, chip-bound updater samples, and the factory XIAO installer; verifies
signatures, generated protocol, standalone UI, partition layout, flash budgets,
and ELF static RAM budgets. Hardware tests are separate and require the exact
selected chip and any requested physical preparation. Gates never flash hardware.

Select `PAGER_BOARD=nice-nano-v2` or `PAGER_BOARD=xiao-nrf52840`. Packages live at
`dist/<board>/<dev|release>/<app|bootloader|updater|installer>`. Each current path is
an atomic link to an immutable directory in `builds/`; use checkpoint copies for
HIL and recovery. An artifact path does not prove what is installed on a chip.

The xtask entry point dispatches to `version`, `execution`, `build`, `package`,
`keys`, `layout`, `checks`, and `ui` modules. Python modules separate package
validation, application USB, raw MSC transport, and serial hardware ownership.
Expected command/build/signing/UF2 errors return `Result` with a CLI error message;
internal image/partition invariants are checked before package publication.

## Versions and provenance

Development display versions are `YY.MM.ordinal-DDHHMMSS`, where the suffix uses
UTC and `ordinal` counts commits in the current month from the main-branch merge
base. Numeric manifest versions are `YY*1000000 + MM*10000 + ordinal` (ordinal
capped at 9999); they are metadata, not an anti-rollback enforcement mechanism.
Bootloader versions include a UTC `YYYYMMDDHHMMSS` suffix. Development timestamps
make otherwise identical builds have different version strings and image hashes.
Release preflight remains restricted to clean `main`, matching production keys,
and no fault/watchdog overrides; production qualification is deferred.

Every package records exact `commit`, `dirty`, `source_tree_sha256`, `build_utc`,
public key fingerprint where relevant, and per-artifact SHA-256 checksums. The
source hash sorts and deduplicates tracked and nonignored paths and hashes path
length, path, presence marker, and content digest. Deleted tracked paths retain a
missing marker. `keys/`, `dist/`, `.venv/`, and `.git/` are excluded. The fingerprint
identifies the checkout at packaging time; subsequent source or report edits change the
checkout hash without changing that saved package. Private keys and bond material
are never included in package metadata. Preserve the existing development key.

## Isolated and offline checks

After fetching dependencies once, warm builds work with `--locked` and offline
Cargo. `CARGO_TARGET_DIR` is honored for compilation and ELF extraction:

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/dist/build-isolated" \
  PAGER_BOARD=xiao-nrf52840 cargo run --locked --target aarch64-apple-darwin \
  -p xtask -- build
```

Use your host Rust target on other hosts. A fresh target directory tests a clean
build without deleting the working tree, keys, checkpoints, or existing build
caches. Record cold/warm timings separately with fixed firmware version inputs
and the same cached dependency sources. Do not interpret a single timing pair as
an improvement. Static RAM checks reserve 16 KiB of physical RAM beyond mapped
ELF RAM segments; this is a budget, not a measured worst-case stack bound.

See [vendor maintenance](VENDOR_PATCHES.md), [DFU](DFU.md), and
[HIL testing](HIL_TESTING.md) for patch and hardware requirements.

The autonomous clients are generated from `web/webusb.html`, `web/ble.html`,
their CSS, and the protocol/session/app JS modules. Edit those sources, then run
`.venv/bin/python tools/generate_protocol.py` and
`cargo run --locked --target aarch64-apple-darwin -p xtask -- build-ui`. The root
HTML files and `dist/ui/` copies embed everything and work without network access.
`check-ui` compares generated artifacts and runs `node --test tests/ui_behavior.cjs`.
The WebUSB/DOM mocks exercise lifecycle, frame correlation, errors, coalesced
refresh, UTF-8 validation, bounded logs and dialog/focus behavior. Actual browser and rendered BLE keyboard checks remain hardware checks.
VoiceOver qualification is excluded by user decision.
