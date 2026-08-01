#!/bin/bash
set -e

# Change directory to the script's directory
cd "$(dirname "$0")"

# Create output distribution directory
mkdir -p dist

echo "=================================================="
echo "         Building nice!nano v2 Firmware           "
echo "=================================================="

echo "1. Compiling release binary..."
cargo build --release

echo "2. Extracting raw Application binary (.bin)..."
cargo objcopy --release -- -O binary target/thumbv7em-none-eabihf/release/pager.bin
cp target/thumbv7em-none-eabihf/release/pager.bin dist/pager.bin

echo "3. Extracting Application HEX (.hex)..."
cargo objcopy --release -- -O ihex target/thumbv7em-none-eabihf/release/pager.hex
cp target/thumbv7em-none-eabihf/release/pager.hex dist/pager.hex

echo "4. Converting Application to USB UF2 (.uf2)..."
python3 uf2conv.py dist/pager.bin --family 0xADA52840 --base 0x00000 --output dist/pager.uf2

echo "=================================================="
echo "🎉 Build complete! Output files generated in dist/:"
echo "--------------------------------------------------"
echo "📂 dist/pager.bin   <- Raw application binary"
echo "                      [Use this for USB Serial DFU Update]"
echo ""
echo "📂 dist/pager.hex   <- Application Intel HEX"
echo ""
echo "📂 dist/pager.uf2   <- Standalone USB UF2 image"
echo "                      [Use this for USB UF2 bootloader flashing]"
echo "=================================================="
