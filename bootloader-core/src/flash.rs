//! Mockable application update engine. Only the application partition is writable.
use crate::codec::{validate_vector_prefix, Layout, Manifest, Uf2Block, UF2_PAYLOAD_SIZE};
use crate::{Observation, UpdateTracker};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    MalformedUf2,
    InconsistentTransfer,
    InvalidManifest,
    InvalidSignature,
    Flash,
    Verification,
    UnsupportedCommand,
}

pub trait Flash {
    fn read(&mut self, address: u32, bytes: &mut [u8]) -> Result<(), Error>;
    fn erase(&mut self, start: u32, end: u32) -> Result<(), Error>;
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Error>;
}

pub trait SignatureVerifier {
    fn verify(&self, message: &[u8; 48], signature: &[u8; 64]) -> bool;
}

pub struct Engine {
    layout: Layout,
    erased: [bool; 256],
    tracker: UpdateTracker,
    manifest: Option<Manifest>,
    complete: bool,
}

impl Engine {
    pub fn new(layout: Layout) -> Result<Self, Error> {
        layout.validate().map_err(|_| Error::InvalidManifest)?;
        Ok(Self {
            layout,
            erased: [false; 256],
            tracker: UpdateTracker::new(),
            manifest: None,
            complete: false,
        })
    }

    pub fn complete(&self) -> bool {
        self.complete
    }

    pub fn accept(
        &mut self,
        block: &Uf2Block,
        flash: &mut impl Flash,
        verifier: &impl SignatureVerifier,
    ) -> Result<(), Error> {
        let payload = block.payload();
        // Inspect a candidate: rejected manifests/counts must not poison recovery.
        let mut candidate = self.tracker.clone();
        let observation = candidate
            .inspect(
                block.block_no,
                block.num_blocks,
                block.target_addr,
                payload.len(),
                self.layout.firmware_start,
                self.layout.storage_start,
                UF2_PAYLOAD_SIZE,
            )
            .map_err(|error| match error {
                crate::TrackError::InconsistentCount
                | crate::TrackError::ConflictingLastPayload => Error::InconsistentTransfer,
                _ => Error::MalformedUf2,
            })?;
        let manifest = if block.block_no == 0 {
            if payload.len() != self.layout.manifest_size as usize
                || payload[Manifest::SIZE..].iter().any(|byte| *byte != 255)
            {
                return Err(Error::InvalidManifest);
            }
            let manifest = Manifest::parse(payload).map_err(|_| Error::InvalidManifest)?;
            if !self.layout.image_len_valid(manifest.image_len) {
                return Err(Error::InvalidManifest);
            }
            if !verifier.verify(&manifest.signed_message(), &manifest.signature) {
                return Err(Error::InvalidSignature);
            }
            if self.manifest.is_some_and(|old| old != manifest) {
                return Err(Error::InconsistentTransfer);
            }
            Some(manifest)
        } else {
            self.manifest
        };
        if let Some(manifest) = manifest {
            let total_len = self.layout.manifest_size + manifest.image_len;
            let count = total_len.div_ceil(UF2_PAYLOAD_SIZE);
            let last_len = total_len - (count - 1) * UF2_PAYLOAD_SIZE;
            if block.num_blocks != count
                || candidate
                    .last_payload_len()
                    .is_some_and(|n| u32::from(n) != last_len)
            {
                return Err(Error::InvalidManifest);
            }
        }
        if observation == Observation::Duplicate {
            let mut existing = [0; 256];
            flash.read(block.target_addr, &mut existing[..payload.len()])?;
            if existing[..payload.len()] != *payload {
                return Err(Error::InconsistentTransfer);
            }
        } else {
            let page = block.target_addr / self.layout.page_size;
            let start = page * self.layout.page_size;
            // Tracker bounds plus page-aligned partition boundaries guarantee
            // the entire erase stays outside bootloader and persistent storage.
            if !self.erased[page as usize] {
                flash.erase(start, start + self.layout.page_size)?;
                self.erased[page as usize] = true;
            }
            flash.write(block.target_addr, payload)?;
            candidate.commit(block.block_no);
        }
        self.tracker = candidate;
        self.manifest = manifest;
        if self.tracker.complete() {
            self.verify(flash, verifier)?;
            self.complete = true;
        }
        Ok(())
    }

    fn verify(
        &self,
        flash: &mut impl Flash,
        verifier: &impl SignatureVerifier,
    ) -> Result<(), Error> {
        let expected = self.manifest.ok_or(Error::InvalidManifest)?;
        let mut bytes = [0; Manifest::SIZE];
        flash.read(self.layout.firmware_start, &mut bytes)?;
        let actual = Manifest::parse(&bytes).map_err(|_| Error::Verification)?;
        if actual != expected || !verifier.verify(&actual.signed_message(), &actual.signature) {
            return Err(Error::Verification);
        }
        let mut vector = [0; 8];
        flash.read(self.layout.image_start, &mut vector)?;
        validate_vector_prefix(&vector, self.layout.image_start, actual.image_len)
            .map_err(|_| Error::Verification)?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 256];
        let mut offset = 0;
        while offset < actual.image_len {
            let length = (actual.image_len - offset).min(buffer.len() as u32) as usize;
            flash.read(self.layout.image_start + offset, &mut buffer[..length])?;
            digest.update(&buffer[..length]);
            offset += length as u32;
        }
        if <[u8; 32]>::from(digest.finalize()) != actual.digest {
            return Err(Error::Verification);
        }
        Ok(())
    }
}
