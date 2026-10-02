import os
from pathlib import Path
import subprocess
import sys

import pytest

from pager_tools.hardware import hardware_lock


def test_lock_exclusion_and_os_release_after_process_death(tmp_path, monkeypatch):
    monkeypatch.setenv("PAGER_LOCK_FILE", str(tmp_path / "hil.lock"))
    script = (
        "from pager_tools.hardware import hardware_lock; import time; "
        "lock=hardware_lock('aabb'); lock.__enter__(); print('locked',flush=True); time.sleep(30)"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", script],
        stdout=subprocess.PIPE,
        text=True,
        cwd=Path(__file__).resolve().parents[1],
        env=os.environ.copy(),
    )
    try:
        assert process.stdout.readline().strip() == "locked"
        with pytest.raises(RuntimeError, match="already used"):
            with hardware_lock("AABB"):
                pytest.fail("must not acquire another process lock")
    finally:
        process.kill()
        process.wait(timeout=5)
        process.stdout.close()
    with hardware_lock("AABB"), hardware_lock("aabb"):
        pass  # Dead process did not leave a stale lock; nested same-thread use works.
