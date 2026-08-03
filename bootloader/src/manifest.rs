use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::public_key::FIRMWARE_SIGNING_PUBLIC_KEYS;

pub const MAGIC: [u8; 8] = *b"PGRFW001";
pub const SLOT_A: u32 = 0;
pub const SLOT_B: u32 = 1;
pub const STATE_PENDING: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Manifest {
    pub state: u32,
    pub magic: [u8; 8],
    pub version: u32,
    pub image_len: u32,
    pub target_slot: u32,
    pub digest: [u8; 32],
    pub signature: [u8; 64],
}

impl Manifest {
    pub fn signed_message(&self) -> [u8; 52] {
        let mut message = [0; 52];
        message[..8].copy_from_slice(&self.magic);
        message[8..12].copy_from_slice(&self.version.to_le_bytes());
        message[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        message[16..20].copy_from_slice(&self.target_slot.to_le_bytes());
        message[20..].copy_from_slice(&self.digest);
        message
    }

    pub fn verifies(&self, image: &[u8], max_image_len: usize) -> bool {
        if self.state != STATE_PENDING
            || self.magic != MAGIC
            || self.image_len == 0
            || self.target_slot > SLOT_B
            || self.image_len as usize > max_image_len
            || self.image_len as usize != image.len()
        {
            return false;
        }
        if Sha256::digest(image).as_slice() != self.digest {
            return false;
        }
        FIRMWARE_SIGNING_PUBLIC_KEYS.iter().any(|bytes| {
            VerifyingKey::from_bytes(bytes).is_ok_and(|key| {
                key.verify_strict(
                    &self.signed_message(),
                    &Signature::from_bytes(&self.signature),
                )
                .is_ok()
            })
        })
    }
}
