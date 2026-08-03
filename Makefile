# ==============================================================================
# Makefile for Pager (nRF52840) firmware
# A/B application behind a 32 KiB secure bootloader. Set SLOT=A or SLOT=B.
# ==============================================================================

-include .env

# Configuration variables (can be overridden from CLI or .env: make flash DEVICE_IP=192.168.42.1)
DEVICE_IP   ?= 192.168.42.1
HTTP_URL    ?= http://$(DEVICE_IP)/update
PORT        ?= $(shell ls /dev/cu.usbmodem* 2>/dev/null | head -n 1)
SLOT        ?= A
TRIAL_NO_CONFIRM ?= 0
WATCHDOG_NO_FEED ?= 0
BIN         ?= dist/pager-$(SLOT).bin
PACKAGE     ?= dist/pager-$(SLOT).signed.pkg
HEX         ?= dist/pager-$(SLOT).hex
ELF         ?= target/thumbv7em-none-eabihf/release/pager
# Monotonic signed-package version; override in release automation if needed.
VERSION     ?= $(shell date +%s)
SIGNING_KEY ?= keys/firmware_signing_private.pem
SIGNING_PUBLIC_KEYS ?= keys/firmware_signing_public.pem keys/firmware_signing_next_public.pem
BOOTLOADER_MANIFEST ?= bootloader/Cargo.toml
BOOTLOADER_RUSTFLAGS := -C link-arg=-Tlink.x
PYTEST      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python -m pytest,python3 -m pytest)
PROBE_RS    ?= probe-rs

.DEFAULT_GOAL := build

.PHONY: all build check clippy fmt verify bootloader sign sign-release verify-package ci ble-client flash flash-http flash-serial flash-swd flash-swd-migration hil test test-hil test-smoke test-dfu test-ble clean clean-dist clean-all help

all: verify build

## 🔨 Build & Quality Targets
build:
	@echo "=================================================="
	@echo "            Building Pager Firmware               "
	@echo "=================================================="
	@PAGER_SLOT=$(SLOT) PAGER_SKIP_TRIAL_CONFIRM=$(TRIAL_NO_CONFIRM) PAGER_SKIP_WATCHDOG_FEED=$(WATCHDOG_NO_FEED) ./build.sh

check:
	@echo "Checking codebase compilation..."
	cargo check --release

clippy:
	@echo "Running Clippy linter..."
	cargo clippy --release -- -D warnings

verify: fmt clippy

bootloader:
	RUSTFLAGS='$(BOOTLOADER_RUSTFLAGS)' cargo build --manifest-path $(BOOTLOADER_MANIFEST) --release

sign: build
	@test -f "$(SIGNING_KEY)" || (echo "Signing key not found: $(SIGNING_KEY)"; exit 2)
	python3 scripts/sign_firmware.py "$(BIN)" --key "$(SIGNING_KEY)" --version "$(VERSION)" --slot "$(SLOT)" --output "$(PACKAGE)"
	$(MAKE) verify-package PACKAGE="$(PACKAGE)"

# A release version is an explicit monotonic release number, never an inferred
# wall-clock timestamp. Keep `sign` convenient for local development.
sign-release: build
	@test -n "$(RELEASE_VERSION)" || (echo "Set RELEASE_VERSION to the next monotonic release number"; exit 2)
	@test -f "$(SIGNING_KEY)" || (echo "Signing key not found: $(SIGNING_KEY)"; exit 2)
	python3 scripts/sign_firmware.py "$(BIN)" --key "$(SIGNING_KEY)" --version "$(RELEASE_VERSION)" --slot "$(SLOT)" --output "$(PACKAGE)"
	$(MAKE) verify-package PACKAGE="$(PACKAGE)"

verify-package:
	@for key in $(SIGNING_PUBLIC_KEYS); do test -f "$$key" || (echo "Trusted public key not found: $$key"; exit 2); done
	python3 scripts/verify_package.py "$(PACKAGE)" $(foreach key,$(SIGNING_PUBLIC_KEYS),--key "$(key)")

# Local, non-destructive equivalent of CI. Hardware tests are intentionally
# excluded; invoke the explicit HIL targets only with a selected physical board.
ci: fmt clippy bootloader
	python3 scripts/verify_layout.py
	rustc --edition=2021 --test src/protocol.rs -o target/protocol-host-tests
	target/protocol-host-tests
	PAGER_SLOT=A ./build.sh
	$(MAKE) sign SLOT=A VERSION=1
	PAGER_SLOT=B ./build.sh
	$(MAKE) sign SLOT=B VERSION=1
	python3 -m compileall -q scripts tests
	$(PYTEST) -q tests/unit

# Web Bluetooth requires a secure context. Chrome treats localhost as trusted,
# unlike the board's plain HTTP control endpoint.
ble-client:
	@echo "Open http://localhost:8000/ble_client.html in Chrome (Ctrl-C to stop)."
	python3 -m http.server 8000 --directory .

fmt:
	@echo "Checking code formatting..."
	cargo fmt --check

## ⚡ Flashing Targets

# Default HTTP OTA target. Ask the running firmware for its inactive slot
# before compiling, so a valid package is never addressed to the active bank.
# Pass SLOT=A or SLOT=B explicitly to flash-http for recovery workflows.
flash:
	@$(MAKE) flash-http SLOT="$$(python3 scripts/select_inactive_slot.py "$(HTTP_URL:/update=/health)")"

# HTTP OTA Flashing (Default method)
flash-http: sign
	@echo "=================================================="
	@echo "        Flashing Firmware via HTTP OTA (Default)  "
	@echo "=================================================="
	@echo "Target URL: $(HTTP_URL)"
	@echo "Package:    $(PACKAGE)"
	@python3 scripts/flash_http.py "$(HTTP_URL)" "$(PACKAGE)"
	@echo ""
	@echo "[+] HTTP OTA flashing complete!"

# USB Serial DFU Flashing (Backup/Reserve method)
flash-serial: sign
	@echo "=================================================="
	@echo "     Flashing Firmware via USB Serial DFU (Backup)"
	@echo "=================================================="
	@test -n "$(PORT)" || (echo "No CDC serial device found; pass PORT=/dev/cu.usbmodem…"; exit 2)
	@python3 scripts/flash_serial.py "$(PORT)" "$(PACKAGE)"

# SWD Flashing via probe-rs (Hardware Programmer / Recovery)
flash-swd: bootloader build
	@echo "=================================================="
	@echo "     Flashing Firmware via SWD Probe (probe-rs)   "
	@echo "=================================================="
	$(PROBE_RS) download --chip nRF52840_xxAA bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader
	$(PROBE_RS) download --chip nRF52840_xxAA --reset $(ELF)

# One-time destructive migration from the legacy single-bank layout.
flash-swd-migration: bootloader sign
	@echo "Erasing the nRF52840 and installing A/B bootloader + Slot $(SLOT)."
	@test "$(SLOT)" = "A" || (echo "Migration must start from SLOT=A"; exit 2)
	$(PROBE_RS) download --chip nRF52840_xxAA --chip-erase --verify bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader
	$(PROBE_RS) download --chip nRF52840_xxAA --binary-format bin --base-address 0x8000 --verify --reset $(PACKAGE)

## 🧪 HIL Testing Targets

# Full HIL test suite
hil: test

test:
	@echo "=================================================="
	@echo "          Running HIL Integration Tests           "
	@echo "=================================================="
	$(PYTEST) --run-hil tests/test_device.py -v

test-hil: test

# Smoke tests (< 25s)
test-smoke:
	@echo "=================================================="
	@echo "            Running HIL Smoke Tests               "
	@echo "=================================================="
	$(PYTEST) --run-hil -m smoke -x -v

# DFU update tests (HTTP + Serial DFU)
test-dfu: sign
	@echo "=================================================="
	@echo "             Running HIL DFU Tests                "
	@echo "=================================================="
	$(PYTEST) --run-hil -m dfu -v --run-destructive

# BLE tests
test-ble:
	@echo "=================================================="
	@echo "             Running HIL BLE Tests                "
	@echo "=================================================="
	$(PYTEST) --run-hil -m ble -v

## 🧹 Cleanup
clean: clean-dist

clean-dist:
	@echo "Cleaning build artifacts..."
	rm -rf dist

clean-all: clean-dist
	cargo clean
	cargo clean --manifest-path $(BOOTLOADER_MANIFEST)

## ❓ Help Target
help:
	@echo "========================================================================"
	@echo "                      Pager Firmware Makefile                          "
	@echo "========================================================================"
	@echo "Usage: make [target] [VARIABLES]"
	@echo ""
	@echo "Available Targets:"
	@echo "  build          Build SLOT=A/B release firmware (.bin, .hex)"
	@echo "  check          Run cargo check for static compilation errors"
	@echo "  clippy         Run cargo clippy linter"
	@echo "  fmt            Check Rust code formatting (cargo fmt)"
	@echo "  bootloader     Build the signed-update bootloader prototype"
	@echo "  sign           Create a development package (VERSION=unix timestamp)"
	@echo "  sign-release   Create a release package (RELEASE_VERSION=<monotonic u32>)"
	@echo "  verify-package Verify package length, digest, and manifest invariants"
	@echo "  ble-client     Serve Web Bluetooth client from trusted localhost:8000"
	@echo "  flash          Flash firmware via HTTP (default flash target)"
	@echo "  flash-http     Flash firmware via HTTP OTA (http://$(DEVICE_IP)/update)"
	@echo "  flash-serial   Flash firmware via USB Serial DFU (backup, $(PORT))"
	@echo "  flash-swd      Flash firmware via SWD programmer (probe-rs recovery)"
	@echo "  flash-swd-migration  Erase chip and install A/B bootloader + signed Slot A"
	@echo "  hil / test     Run full HIL integration test suite"
	@echo "  test-smoke     Run fast HIL smoke test suite (<25s)"
	@echo "  test-dfu       Run HIL DFU update tests (HTTP + Serial DFU)"
	@echo "  test-ble       Run HIL BLE functionality tests"
	@echo "  clean          Clean generated distribution artifacts"
	@echo "  clean-all      Clean Cargo cache and generated distribution artifacts"
	@echo "  help           Show this help message"
	@echo ""
	@echo "Overridable Variables (or place in local .env):"
	@echo "  DEVICE_IP      IP address of board (default: $(DEVICE_IP))"
	@echo "  PORT           Serial port for DFU (default: auto-detected / $(PORT))"
	@echo "  BIN            Path to firmware binary (default: $(BIN))"
	@echo "  SLOT           Application slot to build/sign (A or B; default: $(SLOT))"
	@echo "  TRIAL_NO_CONFIRM=1  HIL-only image that intentionally exercises rollback"
	@echo "========================================================================"
