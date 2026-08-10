# Release procedure

There is no CI during active development. Run locally:

```sh
make quality
make bootloader-release
make build-release
```

A release build must be a clean commit on `main`. Its display version is
`YY.MM.N`, where `N` is the commit's
ordinal in that UTC month. Dirty and non-mainline builds add the UTC build-start
suffix `-DDHHMMSS`; their numeric signed version remains the mainline-derived
`YYMMNNNN`. Feature branches derive `N` from their last reachable mainline commit.

The release signing private key must remain external. Confirm that the release
bootloader contains only the repository release public key and verify the UF2
with xtask before programming hardware. Bootloader key changes require SWD.

All work deliberately deferred until release qualification is tracked in the
repository-level [`RELEASE_TASKS.md`](../RELEASE_TASKS.md). Its blocking items
must be complete before the first public release.
