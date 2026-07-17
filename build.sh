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

echo "4. Converting Application to standalone USB UF2 (.uf2)..."
python3 uf2conv.py dist/pager.bin --family 0xADA52840 --base 0x27000 --output dist/pager.uf2

echo "5. Merging SoftDevice S140 and Application HEX files..."
python3 merge_hex.py

echo "6. Converting combined HEX to initial flash USB UF2 (.uf2)..."
python3 uf2conv.py combined.hex --family 0xADA52840 --output dist/combined.uf2

# Clean up root temp hex
rm -f combined.hex

echo "=================================================="
echo "🎉 Build complete! Output files generated in dist/:"
echo "--------------------------------------------------"
echo "📂 dist/pager.bin   <- Raw application binary"
echo "                      [Use this for Web HTTP OTA Update]"
echo ""
echo "📂 dist/combined.uf2 <- SoftDevice + Application combined"
echo "                      [Use this for FRESH / INITIAL flash]"
echo ""
echo "📂 dist/pager.uf2    <- Standalone application-only UF2"
echo "                      [Use this for USB flash if SD is already installed]"
echo "=================================================="
echo "To flash via USB (UF2):"
echo "1. Double-tap the reset button on your board."
echo "2. Copy the .uf2 file to the mounted NICENANO drive."
echo "=================================================="

