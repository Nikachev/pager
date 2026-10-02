"""OS-released, chip-specific locks and consistent serial discovery."""

from contextlib import contextmanager
import errno
import fcntl
import hashlib
import os
from pathlib import Path
import tempfile
import threading
import time

from serial.tools import list_ports

from .usb import normalize_serial

_HELD_LOCKS = set()


@contextmanager
def hardware_lock(serial):
    identity = normalize_serial(serial)
    if not identity:
        raise ValueError("hardware lock requires chip serial")
    owner = (os.getpid(), threading.get_ident(), identity)
    if owner in _HELD_LOCKS:
        yield  # A flasher called by a HIL test shares that test's ownership.
        return
    suffix = hashlib.sha256(identity.encode()).hexdigest()[:16]
    path = Path(
        os.getenv("PAGER_LOCK_FILE", str(Path(tempfile.gettempdir()) / f"pager-{suffix}.lock"))
    )
    # Never unlink an OS lock: another process may already hold the same inode.
    with path.open("a+", encoding="utf-8") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno in (errno.EACCES, errno.EAGAIN):
                raise RuntimeError(
                    f"chip {identity} is already used by another HIL process"
                ) from error
            raise
        try:
            _HELD_LOCKS.add(owner)
            handle.seek(0)
            handle.truncate()
            handle.write(f"{os.getpid()} {identity}\n")
            handle.flush()
            yield
        finally:
            _HELD_LOCKS.discard(owner)
            fcntl.flock(handle, fcntl.LOCK_UN)


def find_serial_port(timeout=0):
    explicit = os.getenv("SERIAL_PORT") or os.getenv("PORT")
    if explicit:
        return explicit
    wanted = normalize_serial(os.getenv("PAGER_USB_SERIAL"))
    vid = int(os.getenv("PAGER_USB_VID", "0x1209"), 0)
    pid = int(os.getenv("PAGER_USB_PID", "0x0002"), 0)
    deadline = time.monotonic() + timeout
    while True:
        matches = [
            port.device
            for port in list_ports.comports()
            if port.vid == vid
            and port.pid == pid
            and (not wanted or normalize_serial(port.serial_number) == wanted)
        ]
        if len(matches) > 1:
            raise RuntimeError(
                "multiple Pager serial ports; select PAGER_USB_SERIAL or SERIAL_PORT"
            )
        if matches:
            return matches[0]
        if time.monotonic() >= deadline:
            raise RuntimeError("selected Pager serial port not found")
        time.sleep(min(0.25, max(0, deadline - time.monotonic())))
