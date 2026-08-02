#!/usr/bin/env python3
"""Validate the non-secret structural and digest invariants of a Pager package."""

import argparse
import hashlib
import struct
from pathlib import Path

MAGIC = b"PGRFW001"
MANIFEST_PAGE_SIZE = 4096
MANIFEST_FORMAT = "<I8sIII32s64s"
PENDING = 0xFFFF_FFFF
MAX_IMAGE_SIZE = 484 * 1024


def verify(path: Path) -> None:
    package = path.read_bytes()
    manifest_size = struct.calcsize(MANIFEST_FORMAT)
    if len(package) < MANIFEST_PAGE_SIZE:
        raise ValueError("package is shorter than its manifest page")
    state, magic, version, image_len, target_slot, digest, signature = struct.unpack(
        MANIFEST_FORMAT, package[:manifest_size]
    )
    if state != PENDING or magic != MAGIC:
        raise ValueError("package is not a pending Pager firmware manifest")
    if target_slot not in (0, 1) or not 0 < image_len <= MAX_IMAGE_SIZE:
        raise ValueError(f"invalid image length: {image_len}")
    if len(package) != MANIFEST_PAGE_SIZE + image_len:
        raise ValueError("package length does not match manifest image length")
    if package[manifest_size:MANIFEST_PAGE_SIZE] != b"\xff" * (MANIFEST_PAGE_SIZE - manifest_size):
        raise ValueError("manifest page padding is not erased")
    if hashlib.sha256(package[MANIFEST_PAGE_SIZE:]).digest() != digest:
        raise ValueError("image SHA-256 does not match manifest")
    if len(signature) != 64:
        raise ValueError("invalid Ed25519 signature length")
    print(f"Verified {path}: slot={'A' if target_slot == 0 else 'B'} version={version} image={image_len} bytes sha256={digest.hex()}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    verify(parser.parse_args().package)


if __name__ == "__main__":
    main()
