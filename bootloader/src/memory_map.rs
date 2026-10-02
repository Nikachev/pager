//! Memory Map and Flash Partitioning for Pager nRF52840 Bootloader

include!(concat!(env!("OUT_DIR"), "/layout.rs"));
pub const LAYOUT: pager_bootloader_core::codec::Layout = pager_bootloader_core::codec::Layout {
    page_size: PAGE_SIZE,
    bootloader_size: FIRMWARE_START,
    firmware_start: FIRMWARE_START,
    manifest_size: MANIFEST_SIZE,
    image_start: FIRMWARE_START + MANIFEST_SIZE,
    storage_start: FIRMWARE_END,
    storage_size: 0x100000 - FIRMWARE_END,
};
pub fn image_len_is_valid(image_len: u32) -> bool {
    LAYOUT.image_len_valid(image_len)
}
