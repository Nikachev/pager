"""Host-only checks for the signed package verifier."""

import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import sign_firmware  # noqa: E402
import verify_package  # noqa: E402
import flash_serial  # noqa: E402


class ShortWriter:
    def __init__(self) -> None:
        self.data = bytearray()

    def write(self, data: bytes) -> int:
        part = bytes(data[:2])
        self.data.extend(part)
        return len(part)


def test_serial_writer_retries_short_writes() -> None:
    writer = ShortWriter()
    flash_serial.write_all(writer, b"abcdef")
    assert bytes(writer.data) == b"abcdef"


def make_keypair(tmp_path: Path) -> tuple[Path, Path]:
    private_key = tmp_path / "private.pem"
    public_key = tmp_path / "public.pem"
    subprocess.run(["openssl", "genpkey", "-algorithm", "Ed25519", "-out", str(private_key)], check=True)
    subprocess.run(
        ["openssl", "pkey", "-in", str(private_key), "-pubout", "-out", str(public_key)],
        check=True,
    )
    return private_key, public_key


def test_verify_accepts_valid_signed_package(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    private_key, public_key = make_keypair(tmp_path)
    image = tmp_path / "image.bin"
    package = tmp_path / "image.pkg"
    image.write_bytes(b"firmware")
    monkeypatch.setattr(
        sys,
        "argv",
        ["sign_firmware.py", str(image), "--key", str(private_key), "--version", "1", "--slot", "A", "--output", str(package)],
    )
    sign_firmware.main()
    verify_package.verify(package, [public_key])


def test_verify_rejects_tampered_package(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    private_key, public_key = make_keypair(tmp_path)
    image = tmp_path / "image.bin"
    package = tmp_path / "image.pkg"
    image.write_bytes(b"firmware")
    monkeypatch.setattr(
        sys,
        "argv",
        ["sign_firmware.py", str(image), "--key", str(private_key), "--version", "1", "--slot", "A", "--output", str(package)],
    )
    sign_firmware.main()
    tampered = bytearray(package.read_bytes())
    tampered[-1] ^= 1
    package.write_bytes(tampered)
    with pytest.raises(ValueError, match="SHA-256"):
        verify_package.verify(package, [public_key])
