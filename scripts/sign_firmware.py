#!/usr/bin/env python3
"""Create a bootloader package: 4 KiB manifest page followed by firmware."""

import argparse
import hashlib
import struct
import subprocess
import tempfile
from pathlib import Path

MAGIC = b"PGRFW001"
MANIFEST_PAGE_SIZE = 4096
PENDING = 0xFFFF_FFFF
MAX_IMAGE_SIZE = 495616  # 484 KiB; keep in sync with layout.json


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("image", type=Path)
    parser.add_argument("--key", type=Path, default=Path("keys/firmware_signing_private.pem"))
    parser.add_argument("--version", type=int, required=True)
    parser.add_argument("--slot", choices=("A", "B"), required=True)
    parser.add_argument("--output", type=Path, default=Path("dist/pager.signed.pkg"))
    args = parser.parse_args()

    if not 0 < args.version <= 0xFFFF_FFFF:
        raise ValueError("version must be in the range 1..4294967295")

    image = args.image.read_bytes()
    if len(image) > MAX_IMAGE_SIZE:
        raise ValueError(f"Image is {len(image)} bytes; active slot permits {MAX_IMAGE_SIZE}")
    digest = hashlib.sha256(image).digest()
    target_slot = 0 if args.slot == "A" else 1
    message = MAGIC + struct.pack("<III", args.version, len(image), target_slot) + digest
    with tempfile.NamedTemporaryFile() as message_file, tempfile.NamedTemporaryFile() as signature_file:
        message_file.write(message)
        message_file.flush()
        subprocess.run(
            ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(args.key),
             "-in", message_file.name, "-out", signature_file.name],
            check=True,
        )
        signature = Path(signature_file.name).read_bytes()

    if len(signature) != 64:
        raise ValueError(f"Expected a 64-byte Ed25519 signature, got {len(signature)}")
    manifest = struct.pack("<I8sIII32s64s", PENDING, MAGIC, args.version, len(image), target_slot, digest, signature)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    output = manifest + b"\xff" * (MANIFEST_PAGE_SIZE - len(manifest)) + image
    temporary = args.output.with_suffix(args.output.suffix + ".tmp")
    temporary.write_bytes(output)
    temporary.replace(args.output)
    print(f"Wrote {args.output}: slot={args.slot} image={len(image)} bytes version={args.version} sha256={digest.hex()}")


if __name__ == "__main__":
    main()
