//! Memory Map and Flash Partitioning for Pager nRF52840 Bootloader

include!(concat!(env!("OUT_DIR"), "/layout.rs"));
pub const TOTAL_PAGES: usize = ((FIRMWARE_END - FIRMWARE_START) / PAGE_SIZE) as usize;
pub const UF2_PAYLOAD_SIZE: u32 = 256;
pub const MAX_IMAGE_LEN: u32 = FIRMWARE_END - FIRMWARE_START - MANIFEST_SIZE;

pub const fn image_len_is_valid(image_len: u32) -> bool {
    image_len > 0 && image_len <= MAX_IMAGE_LEN
}
