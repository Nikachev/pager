"""
Pytest configuration and shared fixtures for pager device integration tests.
"""

import os
import sys
import fcntl
import time
import pytest
from common import DEFAULT_BASE_URL, DEFAULT_IP, find_serial_port, http_request, wait_for_http_reconnect

LOCK_FILE = "/tmp/pager_hil_test.lock"


def pytest_addoption(parser):
    parser.addoption(
        "--run-destructive",
        action="store_true",
        default=False,
        help="run tests that flash or reboot the physical board",
    )
    parser.addoption(
        "--run-hil",
        action="store_true",
        default=False,
        help="run tests that require the selected physical Pager board",
    )


def pytest_collection_modifyitems(config, items):
    if not config.getoption("--run-hil"):
        skip_hil = pytest.mark.skip(reason="requires --run-hil and a selected physical Pager board")
        for item in items:
            if "hil" in item.keywords:
                item.add_marker(skip_hil)

    if not config.getoption("--run-destructive"):
        skip = pytest.mark.skip(reason="requires --run-destructive")
        for item in items:
            if "dfu" in item.keywords:
                item.add_marker(skip)

    # BLE advertising stops as soon as macOS connects to the HID service.
    # Run every USB/HTTP/OTA assertion first, then run the single continuous
    # BLE session at the end while it owns the radio connection.
    items.sort(key=lambda item: (
        "ble" in item.keywords,
        item.name == "test_serial_logs",
    ))

@pytest.fixture(scope="session", autouse=True)
def lock_hardware_device():
    """Ensure only one Pytest process interacts with the physical hardware at a time."""
    try:
        lock_fd = open(LOCK_FILE, "w")
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except (OSError, IOError):
        pytest.exit("[-] Error: Another HIL test process is already running on the physical device. Concurrent runs are forbidden.", returncode=1)

    yield

    try:
        fcntl.flock(lock_fd, fcntl.LOCK_UN)
        lock_fd.close()
        if os.path.exists(LOCK_FILE):
            os.remove(LOCK_FILE)
    except Exception:
        pass

@pytest.fixture(scope="session")
def repo_root():
    """Returns absolute path to the repository root directory."""
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

@pytest.fixture(scope="session")
def device_ip():
    """Returns target device IP address."""
    return DEFAULT_IP

@pytest.fixture(scope="session")
def base_url():
    """Returns target device base HTTP URL."""
    return DEFAULT_BASE_URL

@pytest.fixture(scope="session")
def serial_port():
    """Returns auto-detected or configured serial port."""
    return find_serial_port()


@pytest.fixture(autouse=True)
def restore_ble_advertising(request):
    """Release a prior BLE central before a BLE test starts scanning."""
    if "ble" not in request.node.keywords:
        return
    assert wait_for_http_reconnect(f"{DEFAULT_BASE_URL}/health", timeout=20), (
        "HTTP control plane did not recover before BLE test"
    )
    response = http_request("POST", "/keyboard/disconnect", retries=4, timeout=5)
    assert response.status == 200
    response.read()
    # The command is consumed by the asynchronous BLE task; allow it to drop
    # the old connection and re-enter advertising before CoreBluetooth scans.
    time.sleep(1.0)
