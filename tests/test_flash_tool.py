import pytest

from tools import flash_uf2


def test_copy_reset_is_success_only_after_get_info(monkeypatch):
    verified = []
    monkeypatch.setattr(flash_uf2, "copy_to_volume", lambda *_: False)
    monkeypatch.setattr(flash_uf2, "wait_for_application", lambda *_: True)
    monkeypatch.setattr(
        flash_uf2, "verify_running_application", lambda *_: verified.append(True)
    )

    flash_uf2.copy_and_verify("pager.uf2", "/Volumes/PAGER_BOOT", object())
    assert verified == [True]


def test_copy_error_without_application_is_failure(monkeypatch):
    monkeypatch.setattr(flash_uf2, "copy_to_volume", lambda *_: False)
    monkeypatch.setattr(flash_uf2, "wait_for_application", lambda *_: False)

    with pytest.raises(RuntimeError, match="copy failed"):
        flash_uf2.copy_and_verify("pager.uf2", "/Volumes/PAGER_BOOT", object())
