# ==============================================================================
# Makefile for Pager (nRF52840) Firmware & Bootloader
# Single-Slot Secure Architecture behind 48 KiB Bootloader
# ==============================================================================

-include .env

export PAGER_BOARD ?= nice-nano-v2
PACKAGE_DIR := dist/$(PAGER_BOARD)/dev
BIN         ?= $(PACKAGE_DIR)/app/pager-signed.bin
UF2         ?= $(PACKAGE_DIR)/app/pager.uf2
BOOTLOADER_ELF := $(PACKAGE_DIR)/bootloader/bootloader.elf
XIAO_INSTALLER_UF2 := dist/xiao-nrf52840/dev/installer/pager-xiao-installer.uf2
PYTEST      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python -m pytest,python3 -m pytest)
PYTHON      ?= $(if $(wildcard .venv/bin/python),.venv/bin/python,python3)
PROBE_RS    ?= probe-rs

HOST_TARGET ?= $(shell rustc -vV | awk '/host:/ {print $$2}')
XTASK       ?= cargo run --locked --target $(HOST_TARGET) --package xtask --

.DEFAULT_GOAL := build

.PHONY: vendor-check bootloader-updater update-bootloader updater-clippy ci quality-fast python-lint clippy-host all build build-release check clippy fmt verify quality bootloader bootloader-release xiao-installer xiao-installer-release install-xiao install-xiao-release test test-hil test-dfu test-flash test-all flash flash-uf2 flash-swd flash-bootloader monitor info clean clean-dist clean-all help

all: verify build

## 🔨 Build & Quality Targets
build:
	@$(XTASK) build

build-release:
	@$(XTASK) build-release

info:
	@$(XTASK) info

monitor:
	@$(PYTHON) tools/monitor_logs.py

check:
	@echo "Checking codebase compilation..."
	cargo check --locked --release --no-default-features --features board-$(PAGER_BOARD)

clippy:
	@echo "Running Clippy linter..."
	cargo clippy --locked --release --no-default-features --features board-$(PAGER_BOARD) -- -D warnings

verify: fmt clippy

bootloader:
	@$(XTASK) bootloader-dev

bootloader-updater:
	@$(XTASK) bootloader-updater-dev

update-bootloader: build bootloader-updater
	@$(PYTHON) tools/update_bootloader.py --updater $(PACKAGE_DIR)/updater/pager.uf2 --restore $(UF2) --board $(PAGER_BOARD) --serial $(PAGER_USB_SERIAL) --report dist/checkpoints/02/$(PAGER_BOARD)/update.json

updater-clippy:
	PAGER_BOOTLOADER_BIN=$(abspath $(PACKAGE_DIR)/bootloader/bootloader.bin) PAGER_BOOTLOADER_HASH=$$($(PYTHON) -c 'import json; print(json.load(open("$(PACKAGE_DIR)/bootloader/package.json"))["image_sha256"])') PAGER_USB_SERIAL=0000000000000000 cargo clippy --locked --manifest-path bootloader-updater/Cargo.toml --release --no-default-features --features board-$(PAGER_BOARD) -- -D warnings

bootloader-release:
	@$(XTASK) bootloader-release

xiao-installer:
	@$(XTASK) xiao-installer-dev

xiao-installer-release:
	@$(XTASK) xiao-installer-release

# Local aliases only; no external CI job is installed during development.
ci: quality

python-lint:
	$(PYTHON) -m ruff check pager_tools tools tests
	$(PYTHON) -m ruff format --check pager_tools tools tests

clippy-host:
	cargo clippy --locked --manifest-path bootloader-updater/Cargo.toml --target $(HOST_TARGET) --lib -- -D warnings
	cargo clippy --locked --target $(HOST_TARGET) -p pager --lib -- -D warnings
	cargo clippy --locked --target $(HOST_TARGET) -p xtask -p pager-bootloader-core --all-targets -- -D warnings

vendor-check:
	$(PYTHON) tools/check_vendor.py

quality-fast: fmt python-lint test clippy-host vendor-check

quality: quality-fast
	PAGER_BOARD=nice-nano-v2 PAGER_USB_SERIAL=0000000000000000 $(XTASK) bootloader-updater-dev
	PAGER_BOARD=xiao-nrf52840 PAGER_USB_SERIAL=0000000000000000 $(XTASK) bootloader-updater-dev
	PAGER_BOARD=nice-nano-v2 $(MAKE) updater-clippy
	PAGER_BOARD=xiao-nrf52840 $(MAKE) updater-clippy
	PAGER_BOARD=nice-nano-v2 $(XTASK) bootloader-dev
	PAGER_BOARD=xiao-nrf52840 $(XTASK) bootloader-dev
	$(XTASK) xiao-installer-dev
	PAGER_BOARD=nice-nano-v2 $(MAKE) clippy
	PAGER_BOARD=xiao-nrf52840 $(MAKE) clippy
	cargo clippy --locked --manifest-path bootloader/Cargo.toml --release --no-default-features --features dev-bootloader,board-nice-nano-v2 -- -D warnings
	cargo clippy --locked --manifest-path bootloader/Cargo.toml --release --no-default-features --features dev-bootloader,board-xiao-nrf52840 -- -D warnings
	PAGER_BOOTLOADER_BIN=$(abspath dist/xiao-nrf52840/dev/bootloader/bootloader.bin) cargo clippy --locked --manifest-path xiao-installer/Cargo.toml --release -- -D warnings
	PAGER_BOARD=nice-nano-v2 $(XTASK) build
	PAGER_BOARD=nice-nano-v2 $(XTASK) verify
	PAGER_BOARD=nice-nano-v2 $(XTASK) size
	PAGER_BOARD=xiao-nrf52840 $(XTASK) build
	PAGER_BOARD=xiao-nrf52840 $(XTASK) verify
	PAGER_BOARD=xiao-nrf52840 $(XTASK) size
	$(XTASK) check-layout
	$(XTASK) check-protocol
	$(XTASK) build-ui
	$(XTASK) check-ui

fmt:
	cargo fmt --manifest-path bootloader-updater/Cargo.toml --check
	cargo fmt --check
	cargo fmt --manifest-path bootloader/Cargo.toml --check
	cargo fmt --manifest-path xiao-installer/Cargo.toml --check
	cargo fmt --manifest-path bootloader/usbd-storage/Cargo.toml --check

## 🧪 Testing Targets

# Non-destructive tests (do NOT flash or reboot hardware)
test:
	cargo test --locked --manifest-path bootloader-updater/Cargo.toml --target $(HOST_TARGET) --lib
	cargo test --locked --target $(HOST_TARGET) --package pager --lib
	cargo test --locked --target $(HOST_TARGET) --package pager-bootloader-core
	cargo test --locked --target $(HOST_TARGET) --package xtask
	cargo test --locked --target $(HOST_TARGET) --package trouble-host --lib --features security,peripheral,gatt,derive,default-packet-pool
	cargo test --locked --manifest-path bootloader/usbd-storage/Cargo.toml --target $(HOST_TARGET) --lib --features bbb,scsi
	cargo test --locked --manifest-path xiao-installer/Cargo.toml --target $(HOST_TARGET) --lib
	$(PYTEST) tests -m "not hil"

test-hil: test
	@echo "=================================================="
	@echo "     Running Non-Destructive Host & Device Tests  "
	@echo "=================================================="
	cargo test --target $(HOST_TARGET) --package xtask
	$(PYTEST) tests/test_device.py --run-hil

# Destructive DFU tests (flashes & reboots hardware)
test-dfu: test-flash

test-flash: build
	@echo "=================================================="
	@echo "     Running DFU Flashing Integration Tests      "
	@echo "=================================================="
	$(PYTEST) tests/test_device.py --run-hil --run-destructive -m dfu

# Complete test suite (non-destructive + DFU flashing tests)
test-all: test build
	@echo "=================================================="
	@echo "        Running Full Suite (All Tests)           "
	@echo "=================================================="
	cargo test --target $(HOST_TARGET) --package xtask
	$(PYTEST) tests/test_device.py --run-hil --run-destructive

## ⚡ Flashing Targets

# Default UF2 USB Flashing
flash: flash-uf2

flash-uf2: build
	@echo "=================================================="
	@echo "        Flashing Firmware via USB UF2             "
	@echo "=================================================="
	$(PYTHON) tools/flash_uf2.py --file $(UF2) --board $(PAGER_BOARD)

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

# One-time destructive migration from the stock Seeed/Adafruit UF2 bootloader.
install-xiao: xiao-installer
	@$(PYTHON) tools/install_xiao_bootloader.py --file $(XIAO_INSTALLER_UF2)

install-xiao-release: xiao-installer-release
	@$(PYTHON) tools/install_xiao_bootloader.py --file dist/xiao-nrf52840/release/installer/pager-xiao-installer.uf2

## 🧹 Cleanup & Helpers

clean:
	cargo clean --manifest-path bootloader-updater/Cargo.toml
	cargo clean
	cargo clean --manifest-path bootloader/Cargo.toml
	cargo clean --manifest-path xiao-installer/Cargo.toml
	cargo clean --manifest-path bootloader/usbd-storage/Cargo.toml

clean-dist:
	rm -rf dist/

clean-all: clean clean-dist

help:
	@echo "Available targets:"
	@echo "  build          Build dev firmware in dist/<board>/dev/app"
	@echo "  bootloader     Build 48 KiB development bootloader"
	@echo "  test           Run non-destructive tests (safe, does NOT flash hardware)"
	@echo "  test-flash     Run DFU flashing tests on physical hardware"
	@echo "  test-all       Run full test suite (non-destructive + DFU flashing)"
	@echo "  flash-uf2      Flash dist/<board>/dev/app/pager.uf2 over USB"
	@echo "  flash-swd      Flash bootloader & firmware via SWD programmer"
	@echo "  xiao-installer Build one-shot UF2 installer for a stock XIAO nRF52840"
	@echo "  install-xiao   Replace stock XIAO bootloader without SWD (destructive)"
	@echo "  check / clippy Check codebase compilation & lints"
