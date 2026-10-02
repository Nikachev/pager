"""
Pytest configuration and shared fixtures for pager device integration tests.
"""

import os
import pytest
from pager_tools.hardware import hardware_lock, find_serial_port


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
    items.sort(
        key=lambda item: (
            "ble" in item.keywords,
            item.name == "test_serial_logs",
        )
    )


@pytest.fixture(scope="session", autouse=True)
def lock_hardware_device(request):
    """Ensure only one Pytest process interacts with the physical hardware at a time."""
    if not request.config.getoption("--run-hil"):
        yield
        return
    import libusb_package
    from pager_tools.usb import find_application

    device = find_application(
        backend=libusb_package.get_libusb1_backend(), serial=os.getenv("PAGER_USB_SERIAL")
    )
    if device is None:
        pytest.exit("selected Pager application not found", returncode=1)
    serial = device.serial_number
    os.environ["PAGER_USB_SERIAL"] = serial
    with hardware_lock(serial):
        yield


@pytest.fixture(scope="session")
def repo_root():
    """Returns absolute path to the repository root directory."""
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


@pytest.fixture(scope="function")
def serial_port():
    """Returns auto-detected or configured serial port, retrying for USB re-enumeration."""
    return find_serial_port(timeout=15)
