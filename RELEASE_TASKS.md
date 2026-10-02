# Pager release tasks

This checklist contains work deliberately deferred from active development. All
items marked **blocking** must be completed before the first public release.

## Security and firmware lifecycle

- [ ] **Blocking:** choose an anti-rollback design and reserve durable flash for
  the highest accepted signed firmware version.
- [ ] Implement atomic anti-rollback updates, power-loss recovery and downgrade
  rejection tests in both the bootloader core and hardware test suite.
- [ ] Perform the release-key ceremony: keep the private key outside the
  repository, verify its public key matches `keys/release-public.hex`, and build
  a release bootloader containing only that public key.
- [ ] Document and rehearse recovery for interrupted UF2, lost signing key and
  compromised signing key scenarios.

## USB identity and compatibility freeze

- [ ] **Blocking:** choose/register the final production VID/PID and update the
  application, bootloader, WebUSB page, Python tools, tests and documentation.
- [ ] Freeze the public USB protocol, manifest, UF2 and persistent-storage
  formats, then define the compatibility and migration policy used after the
  first release.
- [ ] Validate WebUSB and UF2 update behavior with the final USB identity on all
  supported hosts.

## Supported hardware and host matrix

- [x] Validate development firmware on physical Seeed Studio XIAO nRF52840:
  application USB/HID, signed bootloader update, UF2 recovery and persistence.
  Exact images and the final GET_INFO patch qualification scope are recorded in
  [FINAL_VALIDATION.md](docs/FINAL_VALIDATION.md).
- [ ] Validate WebUSB over USB OTG on the supported Android/Chrome version and
  add the result to the support matrix.
- [ ] Repeat the complete two-host BLE acceptance cycle on release candidates:
  macOS in slot 1, Android in slot 2, switching and HID typing in both
  directions, Bluetooth Off/On, reboot and UF2 persistence.
- [ ] Replace or explicitly approve the fixed 13% Battery Service development
  value for the release hardware.

## Automation and release qualification

- [ ] **Blocking:** add CI for formatting, Clippy, host tests, both board builds,
  UI generation, signed-package verification, layout and size budgets.
- [ ] Add an isolated release job that cannot use the development signing key
  and requires the externally supplied release key.
- [ ] Run destructive bootloader/UF2 fault-injection tests on release hardware,
  including incomplete, corrupt, duplicate and reordered blocks and power loss.
- [ ] Complete keyboard and screen-reader smoke testing for both standalone UI
  pages on supported desktop browsers.
- [ ] Produce a clean release build from an exact commit on `main`, verify its
  `YY.MM.N` version, archive build inputs/checksums and run `make quality` plus
  the documented manual acceptance checklist.
- [ ] Review all documentation and remove the active-development warning only
  after formats, identifiers, recovery policy and support matrix are frozen.
