//! Memory Map and Flash Partitioning for Pager nRF52840 Bootloader

pub const FIRMWARE_START: u32 = 0x0000_C000;
pub const FIRMWARE_END: u32 = 0x000F_E000;
pub const PAGE_SIZE: u32 = 4096;
pub const MANIFEST_SIZE: u32 = 256;
pub const TOTAL_PAGES: usize = ((FIRMWARE_END - FIRMWARE_START) / PAGE_SIZE) as usize;

/// Verifies whether the given memory range falls entirely within the application firmware slot
pub fn is_within_firmware_slot(addr: u32, len: u32) -> bool {
    addr >= FIRMWARE_START && (addr + len) <= FIRMWARE_END
}
