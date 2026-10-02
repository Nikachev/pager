import hashlib
import struct
from pathlib import Path

import pytest

from tools import update_bootloader as updater


@pytest.fixture
def prepared(tmp_path, monkeypatch):
    embedded = struct.pack("<II", 0x20040000, 9) + b"\x55" * 40
    digest = hashlib.sha256(embedded).hexdigest()
    serial = "ECA27894EBB268AC"
    descriptor = b"PGRBLUP1" + struct.pack("<Q", int(serial, 16)) + bytes.fromhex(digest)
    descriptor += struct.pack("<II", 1, len(embedded))
    raw = embedded + descriptor
    block = bytearray(512)
    struct.pack_into("<I", block, 16, len(raw))
    block[32 : 32 + len(raw)] = raw
    boot = {
        "board": "nice-nano-v2",
        "kind": "bootloader",
        "version": "test-v1",
        "image_sha256": digest,
        "partition_sha256": hashlib.sha256(
            embedded + b"\xff" * (0xC000 - len(embedded))
        ).hexdigest(),
        "trusted_key_fingerprints": ["test-key"],
    }
    metadata = {
        "board": "nice-nano-v2",
        "mode": "dev",
        "updater": {
            "schema": 1,
            "serial": serial,
            "bootloader": boot,
            "embedded_len": len(embedded),
            "embedded_sha256": digest,
        },
    }
    restore = {"key_fingerprint": "test-key"}
    path = tmp_path / "pager.uf2"
    path.write_bytes(block)
    path.with_name("embedded-bootloader.bin").write_bytes(embedded)
    monkeypatch.setattr(
        updater,
        "load_application_package",
        lambda filename, *_: (
            (bytes(block), metadata) if Path(filename) == path else (b"restore", restore)
        ),
    )
    return path, metadata, restore, serial


def test_exact_signed_descriptor_and_boot_image(prepared):
    path, _, _, serial = prepared
    assert updater.preflight(path, "restore", "nice-nano-v2", serial)[2] == b"restore"


@pytest.mark.parametrize(
    "mutation", ["serial", "board", "key", "digest", "partition", "length", "descriptor"]
)
def test_preflight_refuses_mismatched_target_before_usb(prepared, mutation):
    path, metadata, restore, serial = prepared
    details = metadata["updater"]
    if mutation == "serial":
        details["serial"] = "0" * 16
    elif mutation == "board":
        details["bootloader"]["board"] = "xiao-nrf52840"
    elif mutation == "key":
        restore["key_fingerprint"] = "unknown"
    elif mutation == "digest":
        details["embedded_sha256"] = "0" * 64
    elif mutation == "partition":
        details["bootloader"]["partition_sha256"] = "0" * 64
    elif mutation == "length":
        details["embedded_len"] += 4
    elif mutation == "descriptor":
        details["serial"] = serial = "0" * 16
    with pytest.raises(ValueError):
        updater.preflight(path, "restore", "nice-nano-v2", serial)


def test_boot_verification_requires_every_field_and_capability(prepared):
    _, metadata, _, serial = prepared
    boot = metadata["updater"]["bootloader"]
    fields = {
        "Serial": serial,
        "Board": "nice-nano-v2",
        "Kind": "dev",
        "Version": boot["version"],
        "Partition-SHA256": boot["partition_sha256"],
        "Capabilities": "signed-uf2,bootloader-updater-v1",
    }

    def render(values):
        return "\r\n".join(f"{k}: {v}" for k, v in values.items())

    assert updater.verify_boot_info(render(fields), metadata, serial)["Serial"] == serial
    upper = fields.copy()
    upper["Partition-SHA256"] = fields["Partition-SHA256"].upper()
    assert updater.verify_boot_info(render(upper), metadata, serial)["Serial"] == serial
    for key in fields:
        modified = fields.copy()
        modified[key] = "wrong"
        with pytest.raises(RuntimeError):
            updater.verify_boot_info(render(modified), metadata, serial)


def test_ordinary_flash_refuses_updater_before_usb(monkeypatch):
    from tools import flash_uf2 as flasher

    monkeypatch.setattr(
        flasher, "load_application_package", lambda *_: (b"", {"updater": {"schema": 1}})
    )
    monkeypatch.setattr(flasher, "get_backend", lambda: pytest.fail("USB touched before refusal"))
    with pytest.raises(ValueError, match="dedicated"):
        flasher.flash_uf2("updater.uf2", board="nice-nano-v2")


def test_expected_schema_reset_requires_exact_fresh_state():
    fresh = bytes([5, 0, 0, 255, 255, 0, 0, 0, 0, 0]) + bytes(6) + b"\x05Pager"
    prior = bytes([5, 1, 5, 0, 0, 0, 1, 0, 0, 1]) + bytes(6) + b"\x05Pager"
    with pytest.raises(RuntimeError, match="changed"):
        updater.validate_storage_transition(prior, fresh)
    updater.validate_storage_transition(prior, fresh, expect_reset=True)
    for damaged in [fresh[:9], prior, fresh[:-1], fresh[:-5] + b"Other"]:
        with pytest.raises(RuntimeError):
            updater.validate_storage_transition(prior, damaged, expect_reset=True)


def test_restore_detects_alias_change_but_allows_runtime_link_change():
    before = bytes([5, 1, 5, 0, 0, 0, 1, 0, 0, 1]) + b"alias"
    runtime = bytearray(before)
    runtime[2] = 1
    runtime[4] = 255
    runtime[9] = 0
    updater.validate_storage_transition(before, bytes(runtime))
    with pytest.raises(RuntimeError, match="changed"):
        updater.validate_storage_transition(before, bytes(runtime[:-1]) + b"x")
