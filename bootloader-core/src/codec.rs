//! Shared wire codecs and checked nRF52840 partition/vector rules.

pub const MANIFEST_MAGIC: [u8; 8] = *b"PGRFW002";
pub const UF2_MAGIC_START0: u32 = 0x0A324655;
pub const UF2_MAGIC_START1: u32 = 0x9E5D5157;
pub const UF2_MAGIC_END: u32 = 0x0AB16F30;
pub const UF2_FLAG_FAMILY_ID_PRESENT: u32 = 0x2000;
pub const NRF52840_FAMILY_ID: u32 = 0xADA52840;
pub const UF2_PAYLOAD_SIZE: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecError {
    Length,
    Magic,
    Flags,
    Family,
    Bounds,
    Alignment,
    Vector,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub magic: [u8; 8],
    pub version: u32,
    pub image_len: u32,
    pub digest: [u8; 32],
    pub signature: [u8; 64],
}

impl Manifest {
    pub const SIZE: usize = 112;

    pub fn parse(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() < Self::SIZE {
            return Err(CodecError::Length);
        }
        let manifest = Self {
            magic: bytes[..8].try_into().unwrap(),
            version: word(bytes, 8),
            image_len: word(bytes, 12),
            digest: bytes[16..48].try_into().unwrap(),
            signature: bytes[48..112].try_into().unwrap(),
        };
        if manifest.magic != MANIFEST_MAGIC {
            return Err(CodecError::Magic);
        }
        Ok(manifest)
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..48].copy_from_slice(&self.signed_message());
        bytes[48..].copy_from_slice(&self.signature);
        bytes
    }

    pub fn signed_message(&self) -> [u8; 48] {
        let mut bytes = [0; 48];
        bytes[..8].copy_from_slice(&self.magic);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        bytes[16..].copy_from_slice(&self.digest);
        bytes
    }
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uf2Block {
    pub target_addr: u32,
    pub block_no: u32,
    pub num_blocks: u32,
    payload_size: u32,
    data: [u8; 256],
}

impl Uf2Block {
    pub fn has_magic(bytes: &[u8; 512]) -> bool {
        word(bytes, 0) == UF2_MAGIC_START0 && word(bytes, 4) == UF2_MAGIC_START1
    }

    pub fn parse(bytes: &[u8; 512]) -> Result<Self, CodecError> {
        if !Self::has_magic(bytes) || word(bytes, 508) != UF2_MAGIC_END {
            return Err(CodecError::Magic);
        }
        if word(bytes, 8) != UF2_FLAG_FAMILY_ID_PRESENT {
            return Err(CodecError::Flags);
        }
        if word(bytes, 28) != NRF52840_FAMILY_ID {
            return Err(CodecError::Family);
        }
        let payload_size = word(bytes, 16);
        if payload_size == 0 || payload_size > UF2_PAYLOAD_SIZE || !payload_size.is_multiple_of(4) {
            return Err(CodecError::Length);
        }
        Ok(Self {
            target_addr: word(bytes, 12),
            block_no: word(bytes, 20),
            num_blocks: word(bytes, 24),
            payload_size,
            data: bytes[32..288].try_into().unwrap(),
        })
    }

    pub fn encode(
        target: u32,
        number: u32,
        total: u32,
        payload: &[u8],
    ) -> Result<[u8; 512], CodecError> {
        if payload.is_empty() || payload.len() > 256 || !payload.len().is_multiple_of(4) {
            return Err(CodecError::Length);
        }
        if total == 0 || number >= total || !target.is_multiple_of(4) {
            return Err(CodecError::Bounds);
        }
        let mut bytes = [0; 512];
        for (offset, value) in [
            (0, UF2_MAGIC_START0),
            (4, UF2_MAGIC_START1),
            (8, UF2_FLAG_FAMILY_ID_PRESENT),
            (12, target),
            (16, payload.len() as u32),
            (20, number),
            (24, total),
            (28, NRF52840_FAMILY_ID),
            (508, UF2_MAGIC_END),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[32..32 + payload.len()].copy_from_slice(payload);
        Ok(bytes)
    }

    pub fn payload(&self) -> &[u8] {
        &self.data[..self.payload_size as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub page_size: u32,
    pub bootloader_size: u32,
    pub firmware_start: u32,
    pub manifest_size: u32,
    pub image_start: u32,
    pub storage_start: u32,
    pub storage_size: u32,
}

impl Layout {
    pub fn validate(self) -> Result<(), CodecError> {
        // The installed updater envelope supports exactly this boot region.
        if self.page_size != 4096
            || self.bootloader_size != 0xC000
            || self.firmware_start != self.bootloader_size
            || self.manifest_size != 256
            || self.firmware_start.checked_add(self.manifest_size) != Some(self.image_start)
            || self.storage_start <= self.image_start
            || self.storage_size == 0
            || self
                .storage_start
                .checked_add(self.storage_size)
                .is_none_or(|end| end > 0x100000)
        {
            return Err(CodecError::Bounds);
        }
        if !self.storage_start.is_multiple_of(self.page_size)
            || !self.storage_size.is_multiple_of(self.page_size)
        {
            return Err(CodecError::Alignment);
        }
        Ok(())
    }

    pub fn image_len_valid(self, length: u32) -> bool {
        self.validate().is_ok()
            && length >= 8
            && length.is_multiple_of(4)
            && self
                .image_start
                .checked_add(length)
                .is_some_and(|end| end <= self.storage_start)
    }
}

pub fn validate_vector(image: &[u8], base: u32) -> Result<(), CodecError> {
    let prefix: &[u8; 8] = image
        .get(..8)
        .ok_or(CodecError::Vector)?
        .try_into()
        .unwrap();
    let length = u32::try_from(image.len()).map_err(|_| CodecError::Bounds)?;
    validate_vector_prefix(prefix, base, length)
}

/// Validate a vector prefix when the rest of the image is read in chunks.
pub fn validate_vector_prefix(prefix: &[u8; 8], base: u32, length: u32) -> Result<(), CodecError> {
    let stack = word(prefix, 0);
    let reset = word(prefix, 4);
    if length < 8
        || !(0x2000_0001..=0x2004_0000).contains(&stack)
        || !stack.is_multiple_of(8)
        || reset & 1 == 0
        || base
            .checked_add(length)
            .is_none_or(|end| !(base..end).contains(&(reset & !1)))
    {
        return Err(CodecError::Vector);
    }
    Ok(())
}
