#!/bin/bash
set -e

# Change directory to the script's directory
cd "$(dirname "$0")"

echo "Building release binary..."
cargo build --release

echo "Extracting raw binary..."
cargo objcopy --release -- -O binary target/thumbv7em-none-eabihf/release/pager.bin

echo "Converting to UF2..."
python3 uf2conv.py target/thumbv7em-none-eabihf/release/pager.bin --family 0xADA52840 --base 0x1000 --output target/thumbv7em-none-eabihf/release/pager.uf2

echo "--------------------------------------------------"
echo "Success! UF2 file generated at:"
echo "  target/thumbv7em-none-eabihf/release/pager.uf2"
echo ""
echo "To flash your nice!nano v2:"
echo "1. Double-tap the reset button on your board."
echo "2. Copy the .uf2 file to the mounted drive (e.g. NICENANO or NRF52BOOT)."
echo "--------------------------------------------------"
