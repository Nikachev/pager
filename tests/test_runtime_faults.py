from tools.test_runtime_faults import diagnostic_values, watchdog_reboot_observed


def test_watchdog_proof_requires_new_boot_and_actual_watchdog_cause():
    assert not watchdog_reboot_observed(2, 10000, 20000, 10)
    assert not watchdog_reboot_observed(2, 10000, 34000, 24)
    assert not watchdog_reboot_observed(4, 10000, 2000, 24)
    assert watchdog_reboot_observed(2, 10000, 2000, 24)
    assert watchdog_reboot_observed(2, 1000, 2000, 24)


def test_diagnostic_parser_ignores_history_and_keeps_live_values():
    values = diagnostic_values("DIAG:reset=2;storage_error=1;uptime_ms=345\nBOOT:RESET_REASON:4\n")
    assert values == {"reset": "2", "storage_error": "1", "uptime_ms": "345"}
    assert diagnostic_values("BOOT:RESET_REASON:2") == {}
