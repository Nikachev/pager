# Pager tests

`make test` runs only host tests and requires no board. `make quality` adds
formatting, Clippy, both ARM builds, signed UF2 verification and size/layout
budgets.

Hardware tests are opt-in and serialized with an atomic lock file that does not
depend on `fcntl`:

```sh
make test-hil       # prepared, non-destructive fixture
make test-flash     # explicit destructive UF2/reboot tests
make test-all       # both groups
```

Prepare slot 1 paired to the Mac running pytest and slot 2 paired to another
host. HIL must never erase or re-pair them. A missing board, Bluetooth failure or
wrong fixture is a failure, not a skip. See `docs/HIL_TESTING.md` for the manual
slot-2 typing acceptance that follows the automated run.

Markers are `hil`, `smoke`, `contract`, `ble`, and `dfu`. `dfu` additionally
requires `--run-destructive`. USB selection must be unambiguous; set
`PAGER_USB_SERIAL` or `SERIAL_PORT` when multiple boards are connected.
