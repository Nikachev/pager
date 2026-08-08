# ==============================================================================
# Makefile for Pager (nRF52840) Firmware & Bootloader
# Single-Slot Secure Architecture behind 48 KiB Bootloader
# ==============================================================================

-include .env

PORT        ?= $(shell ls /dev/cu.usbmodem* 2>/dev/null | head -n 1)
BIN         ?= dist/pager-signed.bin
UF2         ?= dist/pager.uf2
ELF         ?= target/thumbv7em-none-eabihf/release/pager
BOOTLOADER_ELF := bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader
BOOTLOADER_MANIFEST := bootloader/Cargo.toml
PYTEST      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python -m pytest,python3 -m pytest)
PYTHON      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python,python3)
PROBE_RS    ?= probe-rs

HOST_TARGET ?= $(shell rustc -vV | awk '/host:/ {print $$2}')
XTASK       ?= cargo run --target $(HOST_TARGET) --package xtask --

.DEFAULT_GOAL := build

.PHONY: all build check clippy fmt verify bootloader test test-dfu test-flash test-all flash flash-uf2 flash-swd flash-bootloader monitor info clean clean-dist clean-all help

all: verify build

## 🔨 Build & Quality Targets
build:
	@$(XTASK) build

info:
	@$(XTASK) info

monitor:
	@$(PYTHON) tools/monitor_logs.py

check:
	@echo "Checking codebase compilation..."
	cargo check --release

clippy:
	@echo "Running Clippy linter..."
	cargo clippy --release -- -D warnings

verify: fmt clippy

bootloader:
	cargo build --manifest-path $(BOOTLOADER_MANIFEST) --release

ci: fmt clippy bootloader
	$(XTASK) build
	$(PYTHON) -m compileall -q scripts tests 2>/dev/null || true

fmt:
	@echo "Checking code formatting..."
	cargo fmt --check

## 🧪 Testing Targets

# Non-destructive tests (do NOT flash or reboot hardware)
test:
	@echo "=================================================="
	@echo "     Running Non-Destructive Host & Device Tests  "
	@echo "=================================================="
	cargo test --target $(HOST_TARGET) --package xtask
	$(PYTEST) tests/test_device.py

# Destructive DFU tests (flashes & reboots hardware)
test-dfu: test-flash

test-flash: build
	@echo "=================================================="
	@echo "     Running DFU Flashing Integration Tests      "
	@echo "=================================================="
	$(PYTEST) tests/test_device.py --run-destructive -m dfu

# Complete test suite (non-destructive + DFU flashing tests)
test-all: build
	@echo "=================================================="
	@echo "        Running Full Suite (All Tests)           "
	@echo "=================================================="
	cargo test --target $(HOST_TARGET) --package xtask
	$(PYTEST) tests/test_device.py --run-destructive

## ⚡ Flashing Targets

# Default UF2 USB Flashing
flash: flash-uf2

flash-uf2: build
	@echo "=================================================="
	@echo "        Flashing Firmware via USB UF2             "
	@echo "=================================================="
	$(PYTHON) tools/flash_uf2.py

# SWD Flashing via probe-rs (Hardware Programmer)
flash-swd: bootloader build
	@echo "=================================================="
	@echo "     Flashing Firmware via SWD Probe (probe-rs)   "
	@echo "=================================================="
	$(PROBE_RS) download --chip nRF52840_xxAA $(BOOTLOADER_ELF)
	$(PROBE_RS) download --chip nRF52840_xxAA --binary-format bin --base-address 0x0000C000 $(BIN)
	$(PROBE_RS) reset --chip nRF52840_xxAA

flash-bootloader: bootloader
	@echo "=================================================="
	@echo "     Flashing Bootloader via SWD (probe-rs)       "
	@echo "=================================================="
	$(PROBE_RS) download --chip nRF52840_xxAA $(BOOTLOADER_ELF)
	$(PROBE_RS) reset --chip nRF52840_xxAA

## 🧹 Cleanup & Helpers

clean:
	cargo clean
	cd bootloader && cargo clean

clean-dist:
	rm -rf dist/

clean-all: clean clean-dist

help:
	@echo "Available targets:"
	@echo "  build          Build single-slot release firmware & dist/pager.uf2"
	@echo "  bootloader     Build 48 KB release bootloader"
	@echo "  test           Run non-destructive tests (safe, does NOT flash hardware)"
	@echo "  test-flash     Run DFU flashing tests on physical hardware"
	@echo "  test-all       Run full test suite (non-destructive + DFU flashing)"
	@echo "  flash-uf2      Flash dist/pager.uf2 over USB"
	@echo "  flash-swd      Flash bootloader & firmware via SWD programmer"
	@echo "  check / clippy Check codebase compilation & lints"
