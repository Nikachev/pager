"""Validate a signed application package before any USB side effect."""

import hashlib
import json
from pathlib import Path
import struct

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

BOARDS = {"nice-nano-v2", "xiao-nrf52840"}
UF2_MAGIC = (0x0A324655, 0x9E5D5157, 0x0AB16F30)
FAMILY = 0xADA52840


def validate_uf2(data, layout, public_keys):
    if not data or len(data) % 512:
        raise ValueError("UF2 length must be a nonzero multiple of 512 bytes")
    count = len(data) // 512
    limit = layout["storage_start"] - layout["firmware_start"]
    if count > (limit + 255) // 256:
        raise ValueError("UF2 exceeds the application partition")
    image = bytearray(count * 256)
    seen = set()
    lengths = {}
    for offset in range(0, len(data), 512):
        block = data[offset : offset + 512]
        magic0, magic1, flags, address, size, number, total, family = struct.unpack_from(
            "<8I", block
        )
        if (magic0, magic1, struct.unpack_from("<I", block, 508)[0]) != UF2_MAGIC:
            raise ValueError("invalid UF2 block magic")
        if flags != 0x2000 or family != FAMILY:
            raise ValueError("invalid UF2 flags or family")
        if total != count or number >= count or number in seen:
            raise ValueError("invalid UF2 block numbering")
        if size < 4 or size > 256 or size % 4 or (number != count - 1 and size != 256):
            raise ValueError("invalid UF2 payload length")
        if (
            address != layout["firmware_start"] + number * 256
            or address + size > layout["storage_start"]
        ):
            raise ValueError("invalid UF2 target address")
        image[number * 256 : number * 256 + size] = block[32 : 32 + size]
        seen.add(number)
        lengths[number] = size
    if image[:8] != b"PGRFW002":
        raise ValueError("invalid signed manifest magic")
    numeric_version, image_len = struct.unpack_from("<II", image, 8)
    manifest_size = layout["manifest_size"]
    full_len = manifest_size + image_len
    if image_len < 8 or full_len > limit or (full_len + 255) // 256 != count:
        raise ValueError("invalid signed image length")
    if lengths[count - 1] != full_len - (count - 1) * 256:
        raise ValueError("UF2 final payload does not match signed image length")
    if image[112:manifest_size] != b"\xff" * (manifest_size - 112):
        raise ValueError("invalid manifest padding")
    raw = bytes(image[manifest_size:full_len])
    digest = hashlib.sha256(raw).digest()
    if digest != image[16:48]:
        raise ValueError("signed image digest mismatch")
    signer = None
    for key in public_keys:
        try:
            Ed25519PublicKey.from_public_bytes(key).verify(bytes(image[48:112]), bytes(image[:48]))
            signer = key
            break
        except InvalidSignature:
            continue
    if signer is None:
        raise ValueError("UF2 signature is not trusted")
    stack, reset = struct.unpack_from("<II", raw)
    start = layout["firmware_start"] + manifest_size
    if not (0x20000000 < stack <= 0x20040000 and stack % 8 == 0):
        raise ValueError("invalid application stack vector")
    if reset & 1 == 0 or not start <= (reset & ~1) < start + image_len:
        raise ValueError("invalid application reset vector")
    return {
        "numeric_version": numeric_version,
        "image_sha256": digest.hex(),
        "uf2_sha256": hashlib.sha256(data).hexdigest(),
        "key_fingerprint": hashlib.sha256(signer).hexdigest(),
    }


def load_application_package(filename, repo_root, board=None):
    filename = Path(filename)
    metadata = json.loads((filename.parent / "package.json").read_text(encoding="utf-8"))
    if metadata.get("schema") != 1 or metadata.get("kind") != "application":
        raise ValueError("not a supported application package")
    if metadata.get("board") not in BOARDS or (board and metadata["board"] != board):
        raise ValueError("package board does not match selected board")
    if metadata.get("mode") not in ("dev", "release") or not metadata.get("version"):
        raise ValueError("missing package mode or version")
    layout = json.loads((Path(repo_root) / "layout.json").read_text())
    if metadata.get("layout") != layout:
        raise ValueError("package layout differs from current tooling")
    protocol = json.loads((Path(repo_root) / "protocol.json").read_text())
    if metadata.get("protocol") != protocol["frame_version"]:
        raise ValueError("package protocol differs from current tooling")
    paths = [Path(repo_root) / "bootloader/firmware_signing_public.hex"]
    if metadata["mode"] == "dev":
        paths.append(Path(repo_root) / "keys/dev_signing_public.hex")
    keys = [bytes.fromhex(path.read_text().strip()) for path in paths if path.exists()]
    data = filename.read_bytes()
    verified = validate_uf2(data, layout, keys)
    for name, value in verified.items():
        if metadata.get(name) != value:
            raise ValueError(f"package metadata mismatch: {name}")
    return data, metadata
