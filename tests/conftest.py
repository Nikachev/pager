"""
Pytest configuration and shared fixtures for pager device integration tests.
"""

import os
import sys
import tempfile
import fcntl
import pytest
from common import find_serial_port

LOCK_FILE = os.environ.get("PAGER_LOCK_FILE", os.path.join(tempfile.gettempdir(), "pager_hil_test.lock"))


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

    # Keep BLE checks last: profile contract tests may briefly switch the active
    # slot, while BLE checks attach to the connection already owned by macOS.
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



@pytest.fixture(scope="function")
def serial_port():
    """Returns auto-detected or configured serial port, retrying for USB re-enumeration."""
    import time
    for _ in range(30):
        port = find_serial_port()
        if port:
            return port
        time.sleep(0.5)
    return find_serial_port()
