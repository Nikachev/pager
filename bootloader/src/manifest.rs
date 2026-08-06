pub const MAGIC: [u8; 8] = *b"PGRFW001";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Manifest {
    pub state: u32,
    pub magic: [u8; 8],
    pub version: u32,
    pub image_len: u32,
    pub reserved: u32,
    pub digest: [u8; 32],
    pub signature: [u8; 64],
}

impl Manifest {
    pub fn signed_message(&self) -> [u8; 52] {
        let mut message = [0; 52];
        message[..8].copy_from_slice(&self.magic);
        message[8..12].copy_from_slice(&self.version.to_le_bytes());
        message[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        message[16..20].copy_from_slice(&self.reserved.to_le_bytes());
        message[20..].copy_from_slice(&self.digest);
        message
    }
}
