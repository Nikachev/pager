#!/bin/bash
set -euo pipefail

# Change directory to the script's directory
cd "$(dirname "$0")"

# Create output distribution directory
mkdir -p dist

echo "=================================================="
echo "              Building Pager Firmware             "
echo "=================================================="

SLOT="${PAGER_SLOT:-A}"
case "$SLOT" in A|B) ;; *) echo "PAGER_SLOT must be A or B" >&2; exit 2 ;; esac
ELF_PATH="target/thumbv7em-none-eabihf/release/pager"
BIN_PATH="dist/pager-${SLOT}.bin"
HEX_PATH="dist/pager-${SLOT}.hex"

echo "1. Compiling release binary..."
PAGER_SLOT="$SLOT" cargo build --release

case "$SLOT" in
    A) EXPECTED_VECTOR=00009000 ;;
    B) EXPECTED_VECTOR=00083000 ;;
esac

# The raw .bin contains no address information, so validate the linked ELF
# before publishing it.  This catches a stale or incorrectly selected memory
# layout rather than producing a package that the bootloader can never boot.
VECTOR_ADDRESS=$(rust-objdump -h "$ELF_PATH" | awk '$2 == ".vector_table" { print $4; exit }')
if [ "$VECTOR_ADDRESS" != "$EXPECTED_VECTOR" ]; then
    echo "Error: slot ${SLOT} vector table is linked at 0x${VECTOR_ADDRESS:-unknown}; expected 0x${EXPECTED_VECTOR}." >&2
    exit 1
fi

extract() {
    local fmt=$1 out=$2
    if command -v rust-objcopy &>/dev/null; then
        rust-objcopy -O "$fmt" "$ELF_PATH" "$out"
    else
        cargo objcopy --release -- -O "$fmt" "$out"
    fi
}

echo "2. Extracting raw Application binary (.bin)..."
extract binary "$BIN_PATH"

echo "3. Extracting Application HEX (.hex)..."
extract ihex "$HEX_PATH"

BIN_SIZE=$(wc -c < "$BIN_PATH" | tr -d ' ')
MAX_BIN_SIZE=$((484 * 1024))
if [ "$BIN_SIZE" -gt "$MAX_BIN_SIZE" ]; then
    echo "Error: ${BIN_PATH} is ${BIN_SIZE} bytes; slot image capacity is ${MAX_BIN_SIZE} bytes." >&2
    exit 1
fi

if command -v shasum >/dev/null 2>&1; then
    BIN_SHA256=$(shasum -a 256 "$BIN_PATH" | awk '{print $1}')
else
    BIN_SHA256=$(sha256sum "$BIN_PATH" | awk '{print $1}')
fi

echo "=================================================="
echo "🎉 Build complete! Output files generated in dist/:"
echo "--------------------------------------------------"
echo "📂 ${BIN_PATH}   <- Raw application binary for slot ${SLOT}"
echo "                      [Input for scripts/sign_firmware.py]"
echo "                      ${BIN_SIZE} bytes, SHA-256: ${BIN_SHA256}"
echo ""
echo "📂 ${HEX_PATH}   <- Application Intel HEX"
echo "=================================================="
