"""Verify the declared local vendor inventory without network access."""

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def check(root=ROOT):
    manifest = json.loads((root / "vendor/PATCHES.json").read_text())
    declared = set()
    for package in manifest["packages"]:
        for entry in package["files"]:
            relative = package["local_path"] + "/" + entry["path"]
            path = root / relative
            declared.add(relative)
            expected = entry["local_sha256"]
            actual = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None
            if actual != expected:
                raise ValueError(f"vendor inventory mismatch: {relative}")
        base = root / package["local_path"]
        for path in base.rglob("*"):
            if not path.is_file() or any(
                part in ("target", ".git", "__pycache__") for part in path.parts
            ):
                continue
            if str(path.relative_to(root)) not in declared:
                raise ValueError(f"undeclared vendor file: {path.relative_to(root)}")
    for entry in manifest["licenses"]:
        path = root / entry["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            raise ValueError(f"vendor license mismatch: {entry['path']}")
    print(f"Vendor inventory: {len(declared)} files and license hashes verified")


if __name__ == "__main__":
    check()
