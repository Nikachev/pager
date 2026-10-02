# Security model

The release bootloader embeds exactly one release public Ed25519 key from the
repository. Its private key is stored separately and supplied only to an explicit
release build. The dev bootloader embeds that release key plus a machine-local
dev public key; its ignored key pair is generated automatically on first use.
An application signed with the dev key cannot boot on release hardware.

The signed, chip-bound development updater can replace a working Pager bootloader
through USB. It is authorized by an application signature trusted by the current
bootloader and validates board, serial, layout, embedded image and ACL before
writing. A signer trusted by a development bootloader therefore has bootloader
replacement privilege, not just application replacement privilege. Production
key rotation policy is deferred; there is no independent online authorization
service. If USB recovery is unavailable, bootloader replacement requires SWD.
Keep power connected during in-place replacement; interruption recovery is not
transactional in this development workflow.

Both bootloaders currently accept any correctly signed numeric version, including
a downgrade. Release anti-rollback is deferred because it needs separately
reserved durable flash. This limitation must be revisited before public release.

WebUSB and CDC trust physical USB access. They are not an authentication boundary.
UF2 validates family, bounds, block metadata, manifest signature and final image
digest before reset. A malformed/incomplete image is not booted and storage is
outside the writable application range.

Diagnostics never expose LTK/IRK or persist raw controller error data that could
contain secret material. Factory reset appends a canonical fresh state; it does
not physically destroy historical bond records. Development storage-erasure and
fault-injection controls are opt-in, forbidden in release builds and excluded
from ordinary app updates. See [STORAGE_FORMAT.md](STORAGE_FORMAT.md).

The factory-XIAO installer is a separate, unsigned one-time entry through a
reviewed factory UF2 bootloader. Host metadata/version checks and on-device ACL
checks bound its supported configuration; they are not a production provisioning
or attestation policy. Unknown factory versions have no override.
