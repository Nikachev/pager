"""
Device flashing script for Pager firmware.
Invokes USB Serial DFU update via scripts/flash_serial.py.
"""

import sys
import os
import subprocess
from common import find_serial_port

def flash(port=None, bin_path="dist/pager.bin"):
    actual_port = port or find_serial_port()
    print(f"[*] Flashing {bin_path} via USB Serial DFU to {actual_port}...")
    script_path = os.path.join(os.path.dirname(__file__), "..", "scripts", "flash_serial.py")
    res = subprocess.run([sys.executable, script_path, actual_port, bin_path])
    return res.returncode == 0

if __name__ == "__main__":
    port_arg = sys.argv[1] if len(sys.argv) > 1 else None
    bin_arg = sys.argv[2] if len(sys.argv) > 2 else "dist/pager.bin"
    success = flash(port_arg, bin_arg)
    sys.exit(0 if success else 1)
