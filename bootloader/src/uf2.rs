//! UF2 (USB Flashing Format) Block Parser
//!
//! UF2 blocks are exactly 512 bytes:
//! - Header: 32 bytes (Magic numbers, flags, target address, payload size, blockNo, numBlocks)
//! - Data: 476 bytes (usually 256 bytes payload)
//! - Final Magic: 4 bytes (0x0AB16F30)

pub const UF2_MAGIC_START0: u32 = 0x0A324655; // "UF2\n"
pub const UF2_MAGIC_START1: u32 = 0x9E5D5157;
pub const UF2_MAGIC_END: u32 = 0x0AB16F30;

pub const UF2_FLAG_FAMILY_ID_PRESENT: u32 = 0x00002000;
pub const NRF52840_FAMILY_ID: u32 = 0xADA52840;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Uf2Block {
    pub magic_start0: u32,
    pub magic_start1: u32,
    pub flags: u32,
    pub target_addr: u32,
    pub payload_size: u32,
    pub block_no: u32,
    pub num_blocks: u32,
    pub family_id: u32,
    pub data: [u8; 476],
    pub magic_end: u32,
}

impl Uf2Block {
    pub fn parse(buf: &[u8; 512]) -> Option<Self> {
        let block: Uf2Block = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Uf2Block) };
        if block.magic_start0 == UF2_MAGIC_START0
            && block.magic_start1 == UF2_MAGIC_START1
            && block.magic_end == UF2_MAGIC_END
        {
            if (block.flags & UF2_FLAG_FAMILY_ID_PRESENT) != 0
                && block.family_id != NRF52840_FAMILY_ID
            {
                return None;
            }
            if block.payload_size as usize <= block.data.len() {
                return Some(block);
            }
        }
        None
    }

    pub fn payload(&self) -> &[u8] {
        &self.data[..self.payload_size as usize]
    }
}
