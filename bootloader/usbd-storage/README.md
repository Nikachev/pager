# Pager usbd-storage fork

Minimal synchronous USB Mass Storage implementation used only by the Pager
bootloader. The fork retains Bulk-Only Transport and the required SCSI command
set. UFI, STM32 examples and unrelated optional integrations were removed.

The public API is intentionally scoped to Pager. Rebase upstream changes only
after the bootloader transaction and macOS/Android UF2 copy tests pass.
