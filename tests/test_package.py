import hashlib
import json
from pathlib import Path
import struct

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
import pytest

from pager_tools.package import load_application_package, validate_uf2


@pytest.fixture
def package(tmp_path):
    layout = json.loads((Path(__file__).resolve().parents[1] / "layout.json").read_text())
    raw = bytearray(600)
    struct.pack_into("<II", raw, 0, 0x20040000, layout["firmware_image_start"] + 9)
    key = Ed25519PrivateKey.from_private_bytes(b"\x42" * 32)
    public = key.public_key().public_bytes_raw()
    digest = hashlib.sha256(raw).digest()
    message = b"PGRFW002" + struct.pack("<II", 26090001, len(raw)) + digest
    image = message + key.sign(message) + b"\xff" * (256 - 112) + raw
    blocks = []
    for number, offset in enumerate(range(0, len(image), 256)):
        chunk = image[offset : offset + 256]
        block = bytearray(512)
        struct.pack_into(
            "<8I",
            block,
            0,
            0x0A324655,
            0x9E5D5157,
            0x2000,
            layout["firmware_start"] + offset,
            len(chunk),
            number,
            (len(image) + 255) // 256,
            0xADA52840,
        )
        block[32 : 32 + len(chunk)] = chunk
        struct.pack_into("<I", block, 508, 0x0AB16F30)
        blocks.append(bytes(block))
    data = b"".join(blocks)
    (tmp_path / "bootloader").mkdir()
    (tmp_path / "keys").mkdir()
    (tmp_path / "bootloader/firmware_signing_public.hex").write_text(
        Ed25519PrivateKey.from_private_bytes(b"\x43" * 32).public_key().public_bytes_raw().hex()
    )
    (tmp_path / "keys/dev_signing_public.hex").write_text(public.hex())
    (tmp_path / "layout.json").write_text(json.dumps(layout))
    (tmp_path / "protocol.json").write_text('{"frame_version":5}')
    metadata = {
        "schema": 1,
        "kind": "application",
        "board": "xiao-nrf52840",
        "mode": "dev",
        "version": "26.09.1",
        "protocol": 5,
        "layout": layout,
        **validate_uf2(data, layout, [public]),
    }
    filename = tmp_path / "pager.uf2"
    filename.write_bytes(data)
    (tmp_path / "package.json").write_text(json.dumps(metadata))
    return filename, layout, public, data


def test_signed_package_and_reordered_blocks(package):
    filename, layout, key, data = package
    _, metadata = load_application_package(filename, filename.parent, "xiao-nrf52840")
    reordered = b"".join(reversed([data[i : i + 512] for i in range(0, len(data), 512)]))
    assert validate_uf2(reordered, layout, [key])["image_sha256"] == metadata["image_sha256"]


@pytest.mark.parametrize(
    "offset,value",
    [
        (0, 0),
        (8, 0),
        (12, 0),
        (16, 0),
        (16, 255),
        (20, 99),
        (24, 99),
        (28, 0),
        (508, 0),
        (512 + 20, 0),
    ],
)
def test_every_block_checked_before_transfer(package, offset, value):
    _, layout, key, data = package
    corrupted = bytearray(data)
    struct.pack_into("<I", corrupted, offset, value)
    with pytest.raises(ValueError):
        validate_uf2(corrupted, layout, [key])


def test_payload_corruption_rejected(package):
    _, layout, key, data = package
    corrupted = bytearray(data)
    corrupted[512 + 50] ^= 1
    with pytest.raises(ValueError, match="digest"):
        validate_uf2(corrupted, layout, [key])


def test_signature_and_wrong_key_rejected(package):
    _, layout, key, data = package
    corrupted = bytearray(data)
    corrupted[32 + 48] ^= 1
    with pytest.raises(ValueError, match="signature"):
        validate_uf2(corrupted, layout, [key])
    with pytest.raises(ValueError, match="signature"):
        validate_uf2(data, layout, [b"\x44" * 32])


@pytest.mark.parametrize(
    "field,value",
    [
        ("board", "nice-nano-v2"),
        ("version", ""),
        ("uf2_sha256", "0" * 64),
        ("numeric_version", 0),
        ("layout", {}),
        ("protocol", 1),
        ("mode", "release"),
    ],
)
def test_package_metadata_mismatch_rejected(package, field, value):
    filename, *_ = package
    path = filename.parent / "package.json"
    metadata = json.loads(path.read_text())
    metadata[field] = value
    path.write_text(json.dumps(metadata))
    with pytest.raises(ValueError):
        load_application_package(filename, filename.parent, "xiao-nrf52840")


def test_truncated_or_extra_blocks_rejected(package):
    _, layout, key, data = package
    for bad in (b"", data[:-1], data[:-512], data + data[:512]):
        with pytest.raises(ValueError):
            validate_uf2(bad, layout, [key])
