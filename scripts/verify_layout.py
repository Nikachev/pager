#!/usr/bin/env python3
"""Check that every consumer agrees with the canonical A/B flash layout."""

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LAYOUT = json.loads((ROOT / "layout.json").read_text())


def require(pattern: str, text: str, expected: int, source: Path) -> None:
    match = re.search(pattern, text)
    if not match:
        raise ValueError(f"missing layout value in {source.relative_to(ROOT)}")
    actual = int(match.group(1).replace("_", ""), 0)
    if actual != expected:
        raise ValueError(
            f"{source.relative_to(ROOT)} has 0x{actual:x}; layout.json requires 0x{expected:x}"
        )


def main() -> None:
    a = (ROOT / "memory_slot_a.x").read_text()
    b = (ROOT / "memory_slot_b.x").read_text()
    flash = (ROOT / "src/flash.rs").read_text()
    sign = (ROOT / "scripts/sign_firmware.py").read_text()
    verify = (ROOT / "scripts/verify_package.py").read_text()

    require(r"FLASH\s*:\s*ORIGIN\s*=\s*(0x[0-9A-Fa-f]+)", a,
            LAYOUT["slot_a_manifest"] + LAYOUT["manifest_page_size"], ROOT / "memory_slot_a.x")
    require(r"FLASH\s*:\s*ORIGIN\s*=\s*(0x[0-9A-Fa-f]+)", b,
            LAYOUT["slot_b_manifest"] + LAYOUT["manifest_page_size"], ROOT / "memory_slot_b.x")
    require(r"STORAGE_START_ADDR:\s*u32\s*=\s*(0x[0-9A-Fa-f_]+)", flash,
            LAYOUT["storage_start"], ROOT / "src/flash.rs")
    require(r"BOOT_CONTROL_PAGE0:\s*u32\s*=\s*(0x[0-9A-Fa-f_]+)", flash,
            LAYOUT["boot_control_page0"], ROOT / "src/flash.rs")
    require(r"BOOT_CONTROL_PAGE1:\s*u32\s*=\s*(0x[0-9A-Fa-f_]+)", flash,
            LAYOUT["boot_control_page1"], ROOT / "src/flash.rs")
    for source, text in ((ROOT / "scripts/sign_firmware.py", sign), (ROOT / "scripts/verify_package.py", verify)):
        require(r"MANIFEST_PAGE_SIZE\s*=\s*(\d+)", text, LAYOUT["manifest_page_size"], source)
        require(r"MAX_IMAGE_SIZE\s*=\s*(\d+)", text, LAYOUT["slot_image_size"], source)
    print("Layout consumers match layout.json")


if __name__ == "__main__":
    main()
