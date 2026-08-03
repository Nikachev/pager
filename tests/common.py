"""Shared constants and helpers for the pager device test-suite.

This module is imported by the unittest-based tests (``test_device.py``) and
can also be reused by the standalone diagnostic/utility scripts under ``tests/``.

Hardware-specific defaults (serial port, board IP, USB volume) live here so they
are defined in exactly one place.
"""

import asyncio
import subprocess
import time
import urllib.request

# ---------------------------------------------------------------------------
# Hardware / device constants
# ---------------------------------------------------------------------------

import os
from serial.tools import list_ports

def find_serial_port():
    env_port = os.getenv("SERIAL_PORT") or os.getenv("PORT")
    if env_port:
        return env_port
    requested_serial = os.getenv("PAGER_USB_SERIAL")
    requested_vid = os.getenv("PAGER_USB_VID")
    requested_pid = os.getenv("PAGER_USB_PID")
    ports = []
    for port in list_ports.comports():
        if not port.device.startswith("/dev/cu.usbmodem"):
            continue
        if requested_serial and port.serial_number != requested_serial:
            continue
        if requested_vid and (port.vid is None or port.vid != int(requested_vid, 0)):
            continue
        if requested_pid and (port.pid is None or port.pid != int(requested_pid, 0)):
            continue
        ports.append(port.device)
    ports.sort()
    if not ports:
        raise RuntimeError("No matching USB modem found")
    if len(ports) != 1:
        raise RuntimeError(f"Multiple USB modems match: {', '.join(ports)}; set SERIAL_PORT")
    return ports[0]

# Keep import side-effect free so HTTP-only/unit tests can run without a board.
# Call `find_serial_port()` from fixtures or the test that actually needs CDC.
DEFAULT_PORT = os.getenv("SERIAL_PORT") or os.getenv("PORT")

# Static IPv4 assigned to the board's CDC-NCM interface (see main.rs IP_ADDRESS).
DEFAULT_IP = os.getenv("DEVICE_IP", "192.168.42.1")
DEFAULT_BASE_URL = os.getenv("DEVICE_URL", f"http://{DEFAULT_IP}")

# Custom 128-bit GATT service and its characteristics (see ble.rs CustomService).
SERVICE_UUID = "9e7a0001-0b3e-46e8-ad30-7746bad7128a"
LED_CHAR_UUID = "9e7a0002-0b3e-46e8-ad30-7746bad7128a"
STATUS_CHAR_UUID = "9e7a0003-0b3e-46e8-ad30-7746bad7128a"

# Standard HID-over-GATT characteristics (see ble.rs HidService).
HID_INPUT_REPORT_UUID = "00002a4d-0000-1000-8000-00805f9b34fb"
HID_BOOT_INPUT_REPORT_UUID = "00002a22-0000-1000-8000-00805f9b34fb"
HID_PROTOCOL_MODE_UUID = "00002a4e-0000-1000-8000-00805f9b34fb"
HID_REPORT_MAP_UUID = "00002a4b-0000-1000-8000-00805f9b34fb"
HID_INFO_UUID = "00002a4a-0000-1000-8000-00805f9b34fb"
HID_CONTROL_POINT_UUID = "00002a4c-0000-1000-8000-00805f9b34fb"
DIS_SERVICE_UUID = "0000180a-0000-1000-8000-00805f9b34fb"
BATTERY_SERVICE_UUID = "0000180f-0000-1000-8000-00805f9b34fb"

# Subsystem log markers emitted by log_msg!() in the firmware. Used to assert
# that the /logs endpoint returns meaningful (not empty/garbage) content.
LOG_MARKERS = ("Web server", "BLE", "DHCP", "SERIAL:", "System heartbeat")


# ---------------------------------------------------------------------------
# Async / network / serial helpers
# ---------------------------------------------------------------------------

def run_async(coro):
    """Run an async coroutine to completion inside synchronous test code."""
    return asyncio.run(coro)


def find_ncm_interface():
    try:
        out = subprocess.check_output(["ifconfig"], text=True)
        current_iface = None
        for line in out.splitlines():
            if line and not line.startswith("\t") and ":" in line:
                current_iface = line.split(":")[0]
            if "02:00:00:00:00:01" in line and current_iface:
                return current_iface
    except Exception:
        pass
    return "en3"


def ncm_down():
    """Tear the NCM host interface down while the board is rebooting."""
    iface = find_ncm_interface()
    try:
        subprocess.run(["sudo", "-n", "ifconfig", iface, "down"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass


def ncm_up():
    """Bring the NCM host interface back up once the board is online."""
    iface = find_ncm_interface()
    try:
        subprocess.run(["networksetup", "-setmanual", "Pager NCM+ACM", "192.168.42.2", "255.255.255.0", "192.168.42.1"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["sudo", "-n", "ifconfig", iface, "192.168.42.2", "netmask", "255.255.255.0", "up"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(3.0)
    except Exception:
        pass


def ensure_ncm_up():
    """Bring the NCM host interface back up if IP is missing or interface is inactive."""
    iface = find_ncm_interface()
    try:
        out = subprocess.check_output(["ifconfig", iface], text=True, stderr=subprocess.DEVNULL)
        if "192.168.42.2" in out and "status: active" in out:
            return
    except Exception:
        pass
    ncm_up()


def wait_for_http_reconnect(url, timeout=30):
    """Poll ``url`` until it returns HTTP 200."""
    ensure_ncm_up()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            res = urllib.request.urlopen(url, timeout=2)
            if res.status == 200:
                time.sleep(0.3)
                return True
        except Exception:
            pass
        time.sleep(0.4)
    return False


def wait_for_serial_reconnect(port=None, timeout=20):
    """Wait until the CDC-ACM serial port reappears after a reboot."""
    import serial
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        actual_port = port or find_serial_port()
        try:
            s = serial.Serial(actual_port, 115200, timeout=1)
            s.close()
            return True
        except Exception:
            pass
        time.sleep(0.5)
    return False


def wait_for_serial_disconnect(port, timeout=10):
    """Wait until the pre-reset CDC device has actually disappeared."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            s = serial.Serial(port, 115200, timeout=0.2)
            s.close()
        except Exception:
            return True
        time.sleep(0.1)
    return False


def _expected_hid_report(c):
    """Mirror of the firmware's ascii_to_hid() so the test is self-validating.

    Returns the 8-byte keyboard report [modifier, reserved, keycode, 0,0,0,0,0]
    for a single character, or None if unmapped.
    """
    modifiers = 0
    if 'a' <= c <= 'z':
        keycode = ord(c) - ord('a') + 0x04
    elif 'A' <= c <= 'Z':
        modifiers = 0x02
        keycode = ord(c) - ord('A') + 0x04
    elif '1' <= c <= '9':
        keycode = ord(c) - ord('1') + 0x1E
    elif c == '0':
        keycode = 0x27
    elif c in '\n\r':
        keycode = 0x28
    elif c == ' ':
        keycode = 0x2C
    elif c == '!':
        modifiers, keycode = 0x02, 0x1E
    elif c == '@':
        modifiers, keycode = 0x02, 0x1F
    elif c == '#':
        modifiers, keycode = 0x02, 0x20
    elif c == '$':
        modifiers, keycode = 0x02, 0x21
    elif c == '%':
        modifiers, keycode = 0x02, 0x22
    elif c == '^':
        modifiers, keycode = 0x02, 0x23
    elif c == '&':
        modifiers, keycode = 0x02, 0x24
    elif c == '*':
        modifiers, keycode = 0x02, 0x25
    elif c == '(':
        modifiers, keycode = 0x02, 0x26
    elif c == ')':
        modifiers, keycode = 0x02, 0x27
    elif c == '-':
        keycode = 0x2D
    elif c == '_':
        modifiers, keycode = 0x02, 0x2D
    elif c == '=':
        keycode = 0x2E
    elif c == '+':
        modifiers, keycode = 0x02, 0x2E
    elif c == '[':
        keycode = 0x2F
    elif c == '{':
        modifiers, keycode = 0x02, 0x2F
    elif c == ']':
        keycode = 0x30
    elif c == '}':
        modifiers, keycode = 0x02, 0x30
    elif c == '\\':
        keycode = 0x31
    elif c == '|':
        modifiers, keycode = 0x02, 0x31
    elif c == ';':
        keycode = 0x33
    elif c == ':':
        modifiers, keycode = 0x02, 0x33
    elif c == '\'':
        keycode = 0x34
    elif c == '"':
        modifiers, keycode = 0x02, 0x34
    elif c == '`':
        keycode = 0x35
    elif c == '~':
        modifiers, keycode = 0x02, 0x35
    elif c == ',':
        keycode = 0x36
    elif c == '<':
        modifiers, keycode = 0x02, 0x36
    elif c == '.':
        keycode = 0x37
    elif c == '>':
        modifiers, keycode = 0x02, 0x37
    elif c == '/':
        keycode = 0x38
    elif c == '?':
        modifiers, keycode = 0x02, 0x38
    else:
        return None
    return bytes([modifiers, 0, keycode, 0, 0, 0, 0, 0])


# ---------------------------------------------------------------------------
# BLE helpers
# ---------------------------------------------------------------------------

async def find_ble_device(service_uuid, timeout=10.0):
    """Scan for the pager device advertising ``service_uuid``.

    Raises RuntimeError if not found (callers typically turn this into a
    skipTest when the device is already connected to another BLE host).
    """
    # Imported lazily so standalone scripts (flash_device.py, etc.) that only
    # need the helpers/constants here don't pay for pulling in bleak.
    from bleak import BleakScanner

    # On macOS, `find_device_by_filter` can miss advertisements that a normal
    # discovery scan receives. Filter the complete discovery result ourselves.
    discovered = await BleakScanner.discover(timeout=timeout, return_adv=True)
    wanted = service_uuid.lower()
    for device, advertisement in discovered.values():
        if wanted in [uuid.lower() for uuid in advertisement.service_uuids]:
            return device
    raise RuntimeError("Could not find BLE device advertising the service UUID")


# ---------------------------------------------------------------------------
# Raw HTTP helpers (for error-path testing)
# ---------------------------------------------------------------------------

def http_host_port(base_url=DEFAULT_BASE_URL):
    """Return (host, port) parsed from a base URL like 'http://192.168.42.1'."""
    rest = base_url.split("//", 1)[1] if "//" in base_url else base_url
    host = rest.split("/", 1)[0]
    return host, 80


def http_request(method, path, data=None, headers=None, retries=4, timeout=5):
    """urllib-based GET/POST that survives the flaky macOS CDC-NCM link.

    The virtual Ethernet interface (en2) occasionally drops — especially after a
    BLE disconnect or a large upload — so a request can fail with
    ``ConnectionRefused`` even though the board's HTTP server is alive. On a
    connection error we bring the interface up and wait for the server to come
    back before retrying. ``HTTPError`` (4xx/5xx) is re-raised immediately,
    since it is a legitimate response the caller wants to assert on.
    """
    url = f"{DEFAULT_BASE_URL}{path}" if path.startswith("/") else f"{DEFAULT_BASE_URL}/{path}"
    last_exc = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
            res = urllib.request.urlopen(req, timeout=timeout)
            time.sleep(0.2)
            return res
        except urllib.error.HTTPError:
            raise
        except Exception as e:
            last_exc = e
            # An interface may still claim to be "active" while the NCM
            # peer drops a TCP SYN after a BLE link transition. Reconfigure
            # it eagerly instead of waiting until the final attempt.
            ncm_up()
            time.sleep(0.4)
    raise last_exc


def raw_http_request(host, port, method, path, body=b"", extra_headers=None,
                     include_content_length=True, body_send_limit=None,
                     close_after_body=False, content_length=None, retries=5):
    """Send a raw HTTP request, optionally omitting/truncating Content-Length.

    ``urllib`` always adds Content-Length and sends the full body, so it cannot
    exercise server error paths (411 missing length, 400 oversized/truncated).
    This helper gives full control over the wire format.

    ``content_length`` overrides the Content-Length header independently of the
    body (used to claim an oversized upload without actually sending it, which
    would otherwise raise ``BrokenPipe`` when the server rejects early).

    Retries on low-level connection errors (the CDC-NCM link is flaky), bringing
    the interface up between attempts. ``HTTPError``-style responses are still
    returned normally.

    Returns ``(status, body_text)``, or ``(None, "")`` if the connection was
    closed before a response could be read (e.g. ``close_after_body=True``).
    """
    import http.client
    last_exc = None
    for attempt in range(retries):
        if attempt > 0:
            time.sleep(0.3)
        conn = http.client.HTTPConnection(host, port, timeout=15)
        try:
            conn.connect()
            conn.putrequest(method, path)
            if extra_headers:
                for k, v in extra_headers.items():
                    conn.putheader(k, v)
            cl = content_length if content_length is not None else len(body)
            if include_content_length:
                conn.putheader("Content-Length", str(cl))
            conn.endheaders()
            to_send = body if body_send_limit is None else body[:body_send_limit]
            if to_send:
                try:
                    conn.send(to_send)
                except (BrokenPipeError, ConnectionResetError, OSError):
                    pass
            if close_after_body:
                conn.close()
                time.sleep(0.2)
                return None, ""
            resp = conn.getresponse()
            status = resp.status
            data = resp.read().decode("utf-8", errors="ignore")
            conn.close()
            time.sleep(0.2)
            return status, data
        except (ConnectionRefusedError, BrokenPipeError, OSError) as e:
            last_exc = e
            try:
                conn.close()
            except Exception:
                pass
            ensure_ncm_up()
            if attempt < retries - 1:
                wait_for_http_reconnect(f"http://{host}:{port}/logs", timeout=5)
        except Exception:
            try:
                conn.close()
            except Exception:
                pass
            raise
    raise last_exc
