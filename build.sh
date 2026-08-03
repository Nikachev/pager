#!/bin/bash
set -euo pipefail

# Change directory to the script's directory
cd "$(dirname "$0")"

HOST_TARGET=$(rustc -vV | awk '/host:/ {print $2}')
cargo run --target "$HOST_TARGET" --package xtask -- build
