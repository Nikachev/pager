# ==============================================================================
# Makefile for Pager (nRF52840) firmware
# A/B application behind a 32 KiB secure bootloader. Set SLOT=A or SLOT=B.
# ==============================================================================

-include .env

# Configuration variables
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
PYTHON      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python,python3)
PROBE_RS    ?= probe-rs

HOST_TARGET ?= $(shell rustc -vV | awk '/host:/ {print $$2}')
XTASK       ?= cargo run --target $(HOST_TARGET) --package xtask --

.DEFAULT_GOAL := build

.PHONY: all build check clippy fmt verify bootloader sign sign-release verify-package ci ble-client flash flash-webusb flash-serial flash-swd hil test test-hil test-smoke test-dfu test-ble clean clean-dist clean-all help

all: verify build

## 🔨 Build & Quality Targets
build:
	@PAGER_SLOT=$(SLOT) PAGER_SKIP_TRIAL_CONFIRM=$(TRIAL_NO_CONFIRM) PAGER_SKIP_WATCHDOG_FEED=$(WATCHDOG_NO_FEED) $(XTASK) build

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
	$(PYTHON) scripts/sign_firmware.py "$(BIN)" --key "$(SIGNING_KEY)" --version "$(VERSION)" --slot "$(SLOT)" --output "$(PACKAGE)"
	$(MAKE) verify-package PACKAGE="$(PACKAGE)"

# A release version is an explicit monotonic release number, never an inferred
# wall-clock timestamp. Keep `sign` convenient for local development.
sign-release: build
	@test -n "$(RELEASE_VERSION)" || (echo "Set RELEASE_VERSION to the next monotonic release number"; exit 2)
	@test -f "$(SIGNING_KEY)" || (echo "Signing key not found: $(SIGNING_KEY)"; exit 2)
	$(PYTHON) scripts/sign_firmware.py "$(BIN)" --key "$(SIGNING_KEY)" --version "$(RELEASE_VERSION)" --slot "$(SLOT)" --output "$(PACKAGE)"
	$(MAKE) verify-package PACKAGE="$(PACKAGE)"

verify-package:
	@for key in $(SIGNING_PUBLIC_KEYS); do test -f "$$key" || (echo "Trusted public key not found: $$key"; exit 2); done
	$(PYTHON) scripts/verify_package.py "$(PACKAGE)" $(foreach key,$(SIGNING_PUBLIC_KEYS),--key "$(key)")

# Local, non-destructive equivalent of CI. Hardware tests are intentionally
# excluded; invoke the explicit HIL targets only with a selected physical board.
ci: fmt clippy bootloader
	$(PYTHON) scripts/verify_layout.py
	rustc --edition=2021 --test src/protocol.rs -o target/protocol-host-tests
	target/protocol-host-tests
	PAGER_SLOT=A ./build.sh
	$(MAKE) sign SLOT=A VERSION=1
	PAGER_SLOT=B ./build.sh
	$(MAKE) sign SLOT=B VERSION=1
	$(PYTHON) -m compileall -q scripts tests
	$(PYTEST) -q tests/unit

# Web Bluetooth requires a secure context. Chrome treats localhost as trusted.
ble-client:
	@echo "Open http://localhost:8000/ble_client.html in Chrome (Ctrl-C to stop)."
	$(PYTHON) -m http.server 8000 --directory .

fmt:
	@echo "Checking code formatting..."
	cargo fmt --check

## ⚡ Flashing Targets

# Default WebUSB target. Query device for its inactive slot before compiling.
flash:
	@slot="$$($(PYTHON) scripts/select_inactive_slot.py)" || exit $$?; \
	case "$$slot" in A|B) ;; *) echo "Invalid inactive slot: $$slot" >&2; exit 2;; esac; \
	echo "Selected inactive slot: $$slot"; \
	$(MAKE) flash-webusb SLOT="$$slot"

# WebUSB DFU Flashing (Default method, Vendor Bulk interface)
flash-webusb: sign
	@echo "=================================================="
	@echo "        Flashing Firmware via WebUSB DFU           "
	@echo "=================================================="
	@echo "Package:    $(PACKAGE)"
	@$(PYTHON) scripts/flash_webusb.py "$(PACKAGE)"
	@echo ""
	@echo "[+] WebUSB DFU flashing complete!"

# USB Serial DFU Flashing (Backup/Reserve method)
flash-serial: sign
	@echo "=================================================="
	@echo "     Flashing Firmware via USB Serial DFU (Backup)"
	@echo "=================================================="
	@test -n "$(PORT)" || (echo "No CDC serial device found; pass PORT=/dev/cu.usbmodem…"; exit 2)
	@$(PYTHON) scripts/flash_serial.py "$(PORT)" "$(PACKAGE)"

# SWD Flashing via probe-rs (Hardware Programmer / Recovery)
flash-swd: bootloader build
	@echo "=================================================="
	@echo "     Flashing Firmware via SWD Probe (probe-rs)   "
	@echo "=================================================="
	$(PROBE_RS) download --chip nRF52840_xxAA bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader
	$(PROBE_RS) download --chip nRF52840_xxAA --reset $(ELF)



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

# DFU update tests (WebUSB + Serial DFU)
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
	@echo "  flash          Flash firmware via WebUSB DFU (default flash target)"
	@echo "  flash-webusb   Flash firmware via WebUSB DFU (vendor bulk interface)"
	@echo "  flash-serial   Flash firmware via USB Serial DFU (backup, $(PORT))"
	@echo "  flash-swd      Flash firmware via SWD programmer (probe-rs recovery)"

	@echo "  hil / test     Run full HIL integration test suite"
	@echo "  test-smoke     Run fast HIL smoke test suite (<25s)"
	@echo "  test-dfu       Run HIL DFU update tests (WebUSB + Serial DFU)"
	@echo "  test-ble       Run HIL BLE functionality tests"
	@echo "  clean          Clean generated distribution artifacts"
	@echo "  clean-all      Clean Cargo cache and generated distribution artifacts"
	@echo "  help           Show this help message"
	@echo ""
	@echo "Overridable Variables (or place in local .env):"
	@echo "  PORT           Serial port for DFU (default: auto-detected / $(PORT))"
	@echo "  BIN            Path to firmware binary (default: $(BIN))"
	@echo "  SLOT           Application slot to build/sign (A or B; default: $(SLOT))"
	@echo "  TRIAL_NO_CONFIRM=1  HIL-only image that intentionally exercises rollback"
	@echo "========================================================================"
