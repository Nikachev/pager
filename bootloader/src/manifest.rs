pub const MAGIC: [u8; 8] = *b"PGRFW002";

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Manifest {
    pub magic: [u8; 8],
    pub version: u32,
    pub image_len: u32,
    pub digest: [u8; 32],
    pub signature: [u8; 64],
}

impl Manifest {
    pub const SIZE: usize = 112;

    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.magic);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        bytes[16..48].copy_from_slice(&self.digest);
        bytes[48..112].copy_from_slice(&self.signature);
        bytes
    }

    pub fn signed_message(&self) -> [u8; 48] {
        let mut message = [0; 48];
        message[..8].copy_from_slice(&self.magic);
        message[8..12].copy_from_slice(&self.version.to_le_bytes());
        message[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        message[16..].copy_from_slice(&self.digest);
        message
    }
}
