import pytest

from tools import flash_uf2
from tools import install_xiao_bootloader


def test_copy_reset_is_success_only_after_get_info(monkeypatch):
    verified = []
    monkeypatch.setattr(flash_uf2, "copy_to_volume", lambda *_: False)
    monkeypatch.setattr(flash_uf2, "wait_for_application", lambda *_: True)
    monkeypatch.setattr(flash_uf2, "verify_running_application", lambda *_: verified.append(True))

    flash_uf2.copy_and_verify("pager.uf2", "/Volumes/PAGER_BOOT", object(), "A" * 16, {})
    assert verified == [True]


def test_copy_error_without_application_is_failure(monkeypatch):
    monkeypatch.setattr(flash_uf2, "copy_to_volume", lambda *_: False)
    monkeypatch.setattr(flash_uf2, "wait_for_application", lambda *_: False)

    with pytest.raises(RuntimeError, match="copy failed"):
        flash_uf2.copy_and_verify("pager.uf2", "/Volumes/PAGER_BOOT", object(), "A" * 16, {})


def test_copy_timeout_does_not_retry(monkeypatch):
    import subprocess

    calls = []

    def run(*args, **kwargs):
        calls.append((args, kwargs))
        raise subprocess.TimeoutExpired(args[0], kwargs["timeout"])

    monkeypatch.setattr(flash_uf2.subprocess, "run", run)
    assert not flash_uf2.copy_to_volume("pager.uf2", "/Volumes/PAGER_BOOT")
    assert len(calls) == 1 and calls[0][1]["timeout"] == 90


@pytest.mark.parametrize("tag,residue,status", [(2, 0, 0), (1, 1, 0), (1, 0, 1), (1, 0, 2)])
def test_csw_mismatch_rejected(tag, residue, status):
    import struct

    with pytest.raises(RuntimeError, match="CSW"):
        flash_uf2.validate_csw(struct.pack("<4sIIB", b"USBS", tag, residue, status), 1)


def test_valid_csw():
    import struct

    flash_uf2.validate_csw(struct.pack("<4sIIB", b"USBS", 1, 0, 0), 1)


def test_custom_usb_identity_and_case_insensitive_serial(monkeypatch):
    from types import SimpleNamespace

    captured = []

    def find(**kwargs):
        captured.append(kwargs)
        return [SimpleNamespace(serial_number="aaaaaaaaaaaaaaaa")]

    monkeypatch.setattr(flash_uf2.usb.core, "find", find)
    device, vid, pid = flash_uf2.find_device(vid=0xAAAA, pid=0xBBBB, serial="A" * 16)
    assert device and (vid, pid) == (0xAAAA, 0xBBBB)
    assert captured[0]["idVendor"] == 0xAAAA and captured[0]["idProduct"] == 0xBBBB


def test_ambiguous_bootloader_refused(monkeypatch):
    from types import SimpleNamespace

    monkeypatch.setattr(
        flash_uf2.usb.core,
        "find",
        lambda **_: [SimpleNamespace(serial_number="a"), SimpleNamespace(serial_number="b")],
    )
    with pytest.raises(RuntimeError, match="multiple"):
        flash_uf2.find_device()


def test_malformed_package_refused_before_usb(monkeypatch, tmp_path):
    monkeypatch.setattr(flash_uf2, "get_backend", lambda: pytest.fail("must not discover USB"))
    with pytest.raises(OSError):
        flash_uf2.flash_uf2(tmp_path / "pager.uf2")


def test_raw_transfer_releases_interface_on_failure(monkeypatch):
    import usb.core
    from types import SimpleNamespace

    calls = []
    device = SimpleNamespace(
        is_kernel_driver_active=lambda _: True,
        detach_kernel_driver=lambda _: calls.append("detach"),
        attach_kernel_driver=lambda _: calls.append("attach"),
        get_active_configuration=lambda: {(0, 0): object()},
    )
    monkeypatch.setattr(flash_uf2.usb.util, "claim_interface", lambda *_: calls.append("claim"))
    monkeypatch.setattr(flash_uf2.usb.util, "release_interface", lambda *_: calls.append("release"))
    monkeypatch.setattr(flash_uf2.usb.util, "dispose_resources", lambda *_: calls.append("dispose"))
    monkeypatch.setattr(flash_uf2.usb.util, "find_descriptor", lambda *_, **__: object())

    def fail(*_):
        raise usb.core.USBError("write failed", errno=5)

    monkeypatch.setattr(flash_uf2, "send_scsi_write_10", fail)
    with pytest.raises(usb.core.USBError):
        flash_uf2.raw_transfer(device, bytes(1024))
    assert calls == ["detach", "claim", "release", "attach", "dispose"]


def test_xiao_installer_rejects_regular_pager_uf2():
    block = bytearray(512)
    block[0:4] = (0x0A324655).to_bytes(4, "little")
    block[4:8] = (0x9E5D5157).to_bytes(4, "little")
    block[8:12] = (0x2000).to_bytes(4, "little")
    block[12:16] = (0x0000C000).to_bytes(4, "little")
    block[16:20] = (256).to_bytes(4, "little")
    block[24:28] = (1).to_bytes(4, "little")
    block[28:32] = (0xADA52840).to_bytes(4, "little")
    block[508:512] = (0x0AB16F30).to_bytes(4, "little")

    with pytest.raises(ValueError, match="target address"):
        install_xiao_bootloader.validate_installer_uf2(bytes(block))


def installer_uf2(payload_lengths=(256, 256)):
    import struct

    blocks = []
    for number, length in enumerate(payload_lengths):
        block = bytearray(512)
        struct.pack_into(
            "<8I",
            block,
            0,
            0x0A324655,
            0x9E5D5157,
            0x2000,
            0x27000 + number * 256,
            length,
            number,
            len(payload_lengths),
            0xADA52840,
        )
        struct.pack_into("<I", block, 508, 0x0AB16F30)
        blocks.append(block)
    return b"".join(blocks)


def test_installer_requires_full_final_block():
    assert install_xiao_bootloader.validate_installer_uf2(installer_uf2()) == 2
    with pytest.raises(ValueError, match="payload length"):
        install_xiao_bootloader.validate_installer_uf2(installer_uf2((256, 44)))


@pytest.mark.parametrize(
    "offset,value", [(8, 0), (8, 0x2001), (16, 0), (16, 257), (16, 44), (20, 1), (24, 3), (28, 0)]
)
def test_installer_rejects_invalid_blocks(offset, value):
    data = bytearray(installer_uf2())
    data[offset : offset + 4] = value.to_bytes(4, "little")
    with pytest.raises(ValueError):
        install_xiao_bootloader.validate_installer_uf2(data)


def test_installer_rejects_factory_bootloader_overlap():
    count = (0xF4000 - 0x27000) // 256 + 1
    with pytest.raises(ValueError, match="overlaps"):
        install_xiao_bootloader.validate_installer_uf2(installer_uf2((256,) * count))


def stock_info(version="0.6.1", board="Seeed_XIAO_nRF52840", sd="S140 version 7.3.0"):
    return (
        f"UF2 Bootloader {version} lib/nrfx (v2.0.0)\r\nBoard-ID: {board}\r\nSoftDevice: {sd}\r\n"
    )


@pytest.mark.parametrize("version", ["0.6.1", "0.6.2"])
@pytest.mark.parametrize("escaped", [False, True])
def test_stock_factory_info_supported(version, escaped):
    info = stock_info(version)
    if escaped:
        info = info.replace("\r\n", "\\r\\n")
    assert install_xiao_bootloader.validate_stock_info(info) == 0x0044


@pytest.mark.parametrize(
    "info",
    [
        stock_info(version="0.9.2"),
        stock_info(board="nRF52840-Feather-revD"),
        stock_info(sd="S140 version 6.1.1"),
        stock_info(sd="not found"),
        "",
    ],
)
def test_incompatible_stock_info_rejected(info):
    with pytest.raises(RuntimeError):
        install_xiao_bootloader.validate_stock_info(info)


def usb_device(serial, pid=0x0044, product="Pager Boot Drive"):
    from types import SimpleNamespace

    return SimpleNamespace(serial_number=serial, idProduct=pid, product=product)


def test_success_requires_matching_chip_and_pager_product(monkeypatch):
    devices = [usb_device("AAAAAAAAAAAAAAAA")]
    monkeypatch.setattr(install_xiao_bootloader.usb.core, "find", lambda **_: devices)
    assert not install_xiao_bootloader.pager_bootloader_ready("BBBBBBBBBBBBBBBB")
    devices[:] = [usb_device("BBBBBBBBBBBBBBBB", product="Other bootloader")]
    assert not install_xiao_bootloader.pager_bootloader_ready("BBBBBBBBBBBBBBBB")
    devices[:] = [usb_device("bbbbbbbbbbbbbbbb")]
    assert install_xiao_bootloader.pager_bootloader_ready("BBBBBBBBBBBBBBBB")


@pytest.mark.parametrize(
    "devices",
    [
        [],
        [usb_device("A" * 16), usb_device("B" * 16)],
        [usb_device("A" * 16, pid=0x0045)],
        [usb_device("invalid")],
    ],
)
def test_ambiguous_or_unidentifiable_target_rejected(monkeypatch, devices):
    monkeypatch.setattr(install_xiao_bootloader.usb.core, "find", lambda **_: devices)
    with pytest.raises(RuntimeError):
        install_xiao_bootloader.stock_usb_serial(0x0044)


def test_unsupported_stock_does_not_copy(monkeypatch, tmp_path):
    source = tmp_path / "installer.uf2"
    source.write_bytes(b"placeholder")
    (tmp_path / "INFO_UF2.TXT").write_text(stock_info(version="0.9.2"))
    monkeypatch.setattr(install_xiao_bootloader, "validate_installer_uf2", lambda _: 1)
    monkeypatch.setattr(
        install_xiao_bootloader,
        "copy_installer",
        lambda *_: pytest.fail("must refuse before copying"),
    )
    with pytest.raises(RuntimeError, match="unsupported stock bootloader"):
        install_xiao_bootloader.install(source, tmp_path)


@pytest.mark.parametrize(
    "ready,copy_ok", [(True, False), (True, True), (False, True), (False, False)]
)
def test_install_requires_target_enumeration(monkeypatch, tmp_path, ready, copy_ok):
    source = tmp_path / "installer.uf2"
    source.write_bytes(b"placeholder")
    (tmp_path / "INFO_UF2.TXT").write_text(stock_info())
    monkeypatch.setattr(install_xiao_bootloader, "validate_installer_uf2", lambda _: 1)
    monkeypatch.setattr(install_xiao_bootloader, "get_backend", lambda: object())
    monkeypatch.setattr(install_xiao_bootloader, "stock_usb_serial", lambda *_: "A" * 16)
    monkeypatch.setattr(install_xiao_bootloader, "copy_installer", lambda *_: copy_ok)
    states = iter([False, ready])

    def target_ready(serial, backend):
        assert serial == "A" * 16
        return next(states)

    monkeypatch.setattr(install_xiao_bootloader, "pager_bootloader_ready", target_ready)
    monkeypatch.setattr(install_xiao_bootloader.time, "sleep", lambda _: None)
    ticks = iter([0, 0, 2])
    monkeypatch.setattr(install_xiao_bootloader.time, "monotonic", lambda: next(ticks))
    if ready:
        install_xiao_bootloader.install(source, tmp_path, timeout=1)
    else:
        with pytest.raises(RuntimeError, match="did not enumerate"):
            install_xiao_bootloader.install(source, tmp_path, timeout=1)
