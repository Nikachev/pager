# Security model

The release bootloader embeds exactly one release public Ed25519 key from the
repository. Its private key is stored separately and supplied only to an explicit
release build. The dev bootloader embeds that release key plus a machine-local
dev public key; its ignored key pair is generated automatically on first use.
An application signed with the dev key cannot boot on release hardware.

Keys change only by replacing the bootloader through SWD. There is no network or
firmware mechanism for key rotation. Losing a release private key therefore
requires a new bootloader and physical recovery; compromise requires replacing
the bootloader key and all affected hardware.

Both bootloaders currently accept any correctly signed numeric version, including
a downgrade. Release anti-rollback is deferred because it needs separately
reserved durable flash. This limitation must be revisited before public release.

WebUSB and CDC trust physical USB access. They are not an authentication boundary.
UF2 validates family, bounds, block metadata, manifest signature and final image
digest before reset. A malformed/incomplete image is not booted and storage is
outside the writable application range.
