extern crate std;
use crate::codec::*;
use crate::flash::{Engine, Error, Flash, SignatureVerifier};
use ed25519_compact::{KeyPair, PublicKey, Seed, Signature};
use sha2::{Digest, Sha256};
use std::{vec, vec::Vec};

const LAYOUT: Layout = Layout {
    page_size: 4096,
    bootloader_size: 0xC000,
    firmware_start: 0xC000,
    manifest_size: 256,
    image_start: 0xC100,
    storage_start: 0xFE000,
    storage_size: 8192,
};
struct Keys(PublicKey);
impl SignatureVerifier for Keys {
    fn verify(&self, message: &[u8; 48], signature: &[u8; 64]) -> bool {
        self.0.verify(message, &Signature::new(*signature)).is_ok()
    }
}
struct Memory {
    bytes: Vec<u8>,
    erased: Vec<u32>,
    writes: usize,
    fail: Option<&'static str>,
    corrupt: bool,
}
impl Memory {
    fn new() -> Self {
        Self {
            bytes: vec![0xA5; 0x100000],
            erased: vec![],
            writes: 0,
            fail: None,
            corrupt: false,
        }
    }
}
impl Flash for Memory {
    fn read(&mut self, address: u32, bytes: &mut [u8]) -> Result<(), Error> {
        if self.fail == Some("read") {
            return Err(Error::Flash);
        }
        bytes.copy_from_slice(&self.bytes[address as usize..address as usize + bytes.len()]);
        if self.corrupt {
            bytes[0] ^= 1;
        }
        Ok(())
    }
    fn erase(&mut self, start: u32, end: u32) -> Result<(), Error> {
        if self.fail == Some("erase") {
            return Err(Error::Flash);
        }
        assert!(start >= LAYOUT.firmware_start && end <= LAYOUT.storage_start);
        if self.fail == Some("partial-erase") {
            self.bytes[start as usize..start as usize + 32].fill(255);
            return Err(Error::Flash);
        }
        self.bytes[start as usize..end as usize].fill(255);
        self.erased.push(start);
        Ok(())
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Error> {
        assert!(
            address >= LAYOUT.firmware_start
                && address as usize + bytes.len() <= LAYOUT.storage_start as usize
        );
        if self.fail == Some("write") {
            return Err(Error::Flash);
        }
        for (index, (target, byte)) in self.bytes[address as usize..address as usize + bytes.len()]
            .iter_mut()
            .zip(bytes)
            .enumerate()
        {
            if self.fail == Some("partial-write") && index == 32 {
                return Err(Error::Flash);
            }
            assert_eq!(*target & byte, *byte, "no zero-to-one flash writes");
            *target &= byte;
        }
        self.writes += 1;
        Ok(())
    }
}
fn fixture(valid_vector: bool) -> (Vec<Uf2Block>, Keys, Vec<u8>) {
    let pair = KeyPair::from_seed(Seed::new([17; 32]));
    let mut image = vec![0x55; 5200];
    image[..4].copy_from_slice(&0x20040000u32.to_le_bytes());
    image[4..8]
        .copy_from_slice(&(LAYOUT.image_start + if valid_vector { 9 } else { 8 }).to_le_bytes());
    let mut manifest = Manifest {
        magic: MANIFEST_MAGIC,
        version: 42,
        image_len: image.len() as u32,
        digest: Sha256::digest(&image).into(),
        signature: [0; 64],
    };
    manifest.signature = pair
        .sk
        .sign(manifest.signed_message(), None)
        .as_ref()
        .try_into()
        .unwrap();
    let mut envelope = manifest.to_bytes().to_vec();
    envelope.resize(256, 255);
    envelope.extend_from_slice(&image);
    let count = envelope.len().div_ceil(256) as u32;
    let blocks = envelope
        .chunks(256)
        .enumerate()
        .map(|(i, payload)| {
            Uf2Block::parse(
                &Uf2Block::encode(
                    LAYOUT.firmware_start + i as u32 * 256,
                    i as u32,
                    count,
                    payload,
                )
                .unwrap(),
            )
            .unwrap()
        })
        .collect();
    (blocks, Keys(pair.pk), envelope)
}
#[test]
fn full_flash_all_rotated_orders_duplicates_digest_and_partition_preservation() {
    let (blocks, keys, envelope) = fixture(true);
    for rotation in 0..blocks.len() {
        let mut engine = Engine::new(LAYOUT).unwrap();
        let mut flash = Memory::new();
        for step in 0..blocks.len() {
            let block = &blocks[(step + rotation) % blocks.len()];
            engine.accept(block, &mut flash, &keys).unwrap();
            let writes = flash.writes;
            engine.accept(block, &mut flash, &keys).unwrap();
            assert_eq!(flash.writes, writes);
            assert_eq!(engine.complete(), step + 1 == blocks.len());
        }
        assert_eq!(
            &flash.bytes
                [LAYOUT.firmware_start as usize..LAYOUT.firmware_start as usize + envelope.len()],
            envelope
        );
        assert!(flash.bytes[..LAYOUT.firmware_start as usize]
            .iter()
            .all(|b| *b == 0xA5));
        assert!(flash.bytes[LAYOUT.storage_start as usize..]
            .iter()
            .all(|b| *b == 0xA5));
        assert_eq!(flash.erased.len(), 2);
        assert_eq!(flash.erased.iter().filter(|p| **p == 0xC000).count(), 1);
        assert_eq!(flash.erased.iter().filter(|p| **p == 0xD000).count(), 1);
    }
}
#[test]
fn invalid_signature_before_erase_does_not_poison_next_valid_transfer() {
    let (blocks, keys, _) = fixture(true);
    let mut engine = Engine::new(LAYOUT).unwrap();
    let mut flash = Memory::new();
    let mut payload = blocks[0].payload().to_vec();
    payload[48] ^= 1;
    let bad = Uf2Block::parse(
        &Uf2Block::encode(LAYOUT.firmware_start, 0, blocks.len() as u32, &payload).unwrap(),
    )
    .unwrap();
    assert_eq!(
        engine.accept(&bad, &mut flash, &keys),
        Err(Error::InvalidSignature)
    );
    assert!(flash.erased.is_empty());
    for block in &blocks {
        engine.accept(block, &mut flash, &keys).unwrap();
    }
    assert!(engine.complete());
}
#[test]
fn omitted_conflicting_repeated_and_corrupt_image_never_complete() {
    let (blocks, keys, _) = fixture(true);
    let mut engine = Engine::new(LAYOUT).unwrap();
    let mut flash = Memory::new();
    for block in &blocks[..blocks.len() - 1] {
        engine.accept(block, &mut flash, &keys).unwrap();
    }
    assert!(!engine.complete());
    let mut payload = blocks[1].payload().to_vec();
    payload[25] ^= 1;
    let bad = Uf2Block::parse(
        &Uf2Block::encode(blocks[1].target_addr, 1, blocks.len() as u32, &payload).unwrap(),
    )
    .unwrap();
    assert_eq!(
        engine.accept(&bad, &mut flash, &keys),
        Err(Error::InconsistentTransfer)
    );
    flash.bytes[LAYOUT.image_start as usize + 16] ^= 1;
    assert_eq!(
        engine.accept(blocks.last().unwrap(), &mut flash, &keys),
        Err(Error::Verification)
    );
    assert!(!engine.complete());
}
#[test]
fn valid_signature_with_invalid_vector_never_completes() {
    let (blocks, keys, _) = fixture(false);
    let mut engine = Engine::new(LAYOUT).unwrap();
    let mut flash = Memory::new();
    for block in &blocks[..blocks.len() - 1] {
        engine.accept(block, &mut flash, &keys).unwrap();
    }
    assert_eq!(
        engine.accept(blocks.last().unwrap(), &mut flash, &keys),
        Err(Error::Verification)
    );
    assert!(!engine.complete());
}
#[test]
fn flash_failures_stop_and_allow_identical_retry() {
    let (blocks, keys, _) = fixture(true);
    for operation in ["erase", "write", "partial-erase", "partial-write"] {
        let mut engine = Engine::new(LAYOUT).unwrap();
        let mut flash = Memory::new();
        flash.fail = Some(operation);
        assert_eq!(
            engine.accept(&blocks[0], &mut flash, &keys),
            Err(Error::Flash)
        );
        assert!(!engine.complete());
        flash.fail = None;
        for block in &blocks {
            engine.accept(block, &mut flash, &keys).unwrap();
        }
        assert!(engine.complete());
    }
    let mut engine = Engine::new(LAYOUT).unwrap();
    let mut flash = Memory::new();
    for block in &blocks[..blocks.len() - 1] {
        engine.accept(block, &mut flash, &keys).unwrap();
    }
    flash.fail = Some("read");
    assert_eq!(
        engine.accept(blocks.last().unwrap(), &mut flash, &keys),
        Err(Error::Flash)
    );
    assert!(!engine.complete());
    flash.fail = None;
    engine
        .accept(blocks.last().unwrap(), &mut flash, &keys)
        .unwrap();
    assert!(engine.complete());
}
#[test]
fn codec_checks_flags_family_lengths_layout_and_vector_bounds() {
    let mut packet = Uf2Block::encode(0xC000, 0, 1, &[255; 256]).unwrap();
    for (offset, value, error) in [
        (8, 0u32, CodecError::Flags),
        (28, 0, CodecError::Family),
        (16, 0, CodecError::Length),
        (16, 260, CodecError::Length),
        (16, 3, CodecError::Length),
    ] {
        let original = packet[offset..offset + 4].to_vec();
        packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(Uf2Block::parse(&packet), Err(error));
        packet[offset..offset + 4].copy_from_slice(&original);
    }
    assert_eq!(Manifest::parse(&[0; 111]), Err(CodecError::Length));
    for layout in [
        Layout {
            storage_start: 0xC000,
            ..LAYOUT
        },
        Layout {
            storage_size: u32::MAX,
            ..LAYOUT
        },
        Layout {
            image_start: 0xC104,
            ..LAYOUT
        },
        Layout {
            bootloader_size: 0xD000,
            ..LAYOUT
        },
        Layout {
            storage_start: 0xFDFFF,
            ..LAYOUT
        },
    ] {
        assert!(layout.validate().is_err());
    }
    let mut vector = [0; 8];
    vector[..4].copy_from_slice(&0x20040000u32.to_le_bytes());
    vector[4..].copy_from_slice(&0xC109u32.to_le_bytes());
    assert_eq!(validate_vector_prefix(&vector, 0xC100, 16), Ok(()));
    assert_eq!(
        validate_vector_prefix(&vector, 0xC100, 8),
        Err(CodecError::Vector)
    );
    vector[0] = 4;
    assert_eq!(
        validate_vector_prefix(&vector, 0xC100, 16),
        Err(CodecError::Vector)
    );
}
#[test]
fn bounded_malformed_addresses_counts_and_final_lengths_leave_partitions_untouched() {
    let (blocks, keys, _) = fixture(true);
    for address in [0, 0xBFFF, 0xC004, 0xFE000, u32::MAX - 3] {
        let mut engine = Engine::new(LAYOUT).unwrap();
        let mut flash = Memory::new();
        let bad =
            Uf2Block::encode(address & !3, 0, blocks.len() as u32, blocks[0].payload()).unwrap();
        assert!(engine
            .accept(&Uf2Block::parse(&bad).unwrap(), &mut flash, &keys)
            .is_err());
        assert!(flash.erased.is_empty());
    }
    let mut engine = Engine::new(LAYOUT).unwrap();
    let mut flash = Memory::new();
    engine.accept(&blocks[0], &mut flash, &keys).unwrap();
    let last = blocks.last().unwrap();
    let wrong = Uf2Block::parse(
        &Uf2Block::encode(last.target_addr, last.block_no, last.num_blocks, &[0; 256]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        engine.accept(&wrong, &mut flash, &keys),
        Err(Error::InvalidManifest)
    );
    assert!(!engine.complete());
}
