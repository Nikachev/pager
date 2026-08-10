//! Secure UF2 flashing engine for nRF52840 NVMC.
//!
//! Blocks may arrive out of order and may be repeated by an MSC host. Each flash
//! page is erased once, while a bitmap records unique UF2 blocks. Firmware is
//! never executed until the signed manifest and the complete image verify.

use crate::fat16::get_virtual_fat_sector;
use crate::memory_map::{
    image_len_is_valid, FIRMWARE_END, FIRMWARE_START, MANIFEST_SIZE, PAGE_SIZE, TOTAL_PAGES,
    UF2_PAYLOAD_SIZE,
};
use crate::scsi::{handle_scsi_inquiry, handle_scsi_read_capacity};
use crate::uf2::Uf2Block;

use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::NorFlash;
use sha2::{Digest, Sha256};
use pager_bootloader_core::{Observation, UpdateTracker};
use usb_device::bus::UsbBus;
use usbd_storage::subclass::scsi::{Scsi, ScsiCommand};
use usbd_storage::transport::bbb::BulkOnly;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    MalformedUf2,
    InconsistentTransfer,
    InvalidManifest,
    InvalidSignature,
    Flash,
    Verification,
    UnsupportedCommand,
}

impl UpdateError {
    fn sense(self) -> (u8, u8, u8) {
        match self {
            Self::MalformedUf2 | Self::InconsistentTransfer | Self::InvalidManifest => {
                (0x05, 0x24, 0x00) // ILLEGAL REQUEST / invalid field in CDB/data
            }
            Self::InvalidSignature | Self::Verification => {
                (0x03, 0x11, 0x00) // MEDIUM ERROR / unrecovered read error
            }
            Self::Flash => (0x03, 0x0C, 0x02), // write error
            Self::UnsupportedCommand => (0x05, 0x20, 0x00),
        }
    }
}

pub struct Uf2FlashEngine<'a> {
    nvmc: Nvmc<'a>,
    erased_pages: [bool; TOTAL_PAGES],
    tracker: UpdateTracker,
    manifest: Option<crate::manifest::Manifest>,
    write_block_buf: [u8; 512],
    write_buf_off: usize,
    write_sectors_done: usize,
    current_write_lba: Option<u64>,
    last_error: Option<UpdateError>,
    reset_pending: bool,
}

impl<'a> Uf2FlashEngine<'a> {
    pub fn new(nvmc: Nvmc<'a>) -> Self {
        Self {
            nvmc,
            erased_pages: [false; TOTAL_PAGES],
            tracker: UpdateTracker::new(),
            manifest: None,
            write_block_buf: [0; 512],
            write_buf_off: 0,
            write_sectors_done: 0,
            current_write_lba: None,
            last_error: None,
            reset_pending: false,
        }
    }

    pub fn take_reset_pending(&mut self) -> bool {
        core::mem::take(&mut self.reset_pending)
    }

    pub fn handle_scsi_command<'alloc, Bus: UsbBus + 'alloc, Buf: core::borrow::BorrowMut<[u8]>>(
        &mut self,
        mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>,
    ) {
        match cmd.kind {
            ScsiCommand::TestUnitReady => cmd.pass(),
            ScsiCommand::StartStop { .. } | ScsiCommand::SynchronizeCache => cmd.pass(),
            ScsiCommand::Inquiry { alloc_len, .. } => handle_scsi_inquiry(cmd, alloc_len),
            ScsiCommand::ReadCapacity10 => handle_scsi_read_capacity(cmd),
            ScsiCommand::ReadCapacity16 { alloc_len } => {
                let mut resp = [0u8; 32];
                resp[0..8].copy_from_slice(&131071u64.to_be_bytes());
                resp[8..12].copy_from_slice(&512u32.to_be_bytes());
                let send_len = (alloc_len as usize).min(resp.len());
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::ReadFormatCapacities { alloc_len } => {
                let mut resp = [0u8; 12];
                resp[3] = 8;
                resp[4..8].copy_from_slice(&131072u32.to_be_bytes());
                resp[8] = 0x02;
                resp[9..12].copy_from_slice(&[0x00, 0x02, 0x00]);
                let send_len = (alloc_len as usize).min(resp.len());
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::Read { lba, len } => {
                let mut sector = [0u8; 512];
                for offset in 0..len {
                    let Some(cur_lba) = lba.checked_add(offset).and_then(|v| u32::try_from(v).ok())
                    else {
                        self.fail(cmd, UpdateError::MalformedUf2);
                        return;
                    };
                    get_virtual_fat_sector(cur_lba, &mut sector);
                    let mut written = 0;
                    while written < sector.len() {
                        match cmd.write_data(&sector[written..]) {
                            Ok(n) if n > 0 => written += n,
                            _ => return,
                        }
                    }
                }
                cmd.pass();
            }
            ScsiCommand::Write { lba, len } => self.handle_write(cmd, lba, len),
            ScsiCommand::ModeSense6 { alloc_len, .. } => {
                let resp = [0x03, 0, 0, 0];
                let send_len = (alloc_len as usize).min(resp.len());
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::ModeSense10 { alloc_len, .. } => {
                let resp = [0, 6, 0, 0, 0, 0, 0, 0];
                let send_len = (alloc_len as usize).min(resp.len());
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::RequestSense { alloc_len, .. } => {
                let mut resp = [0u8; 18];
                resp[0] = 0x70;
                resp[7] = 10;
                if let Some(error) = self.last_error.take() {
                    let (key, asc, ascq) = error.sense();
                    resp[2] = key;
                    resp[12] = asc;
                    resp[13] = ascq;
                }
                let send_len = (alloc_len as usize).min(resp.len());
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::Unknown => self.fail(cmd, UpdateError::UnsupportedCommand),
        }
    }

    fn handle_write<'alloc, Bus: UsbBus + 'alloc, Buf: core::borrow::BorrowMut<[u8]>>(
        &mut self,
        mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>,
        lba: u64,
        len: u64,
    ) {
        let Ok(total_sectors) = usize::try_from(len) else {
            self.fail(cmd, UpdateError::MalformedUf2);
            return;
        };
        if self.current_write_lba != Some(lba) {
            self.current_write_lba = Some(lba);
            self.write_buf_off = 0;
            self.write_sectors_done = 0;
        }

        while self.write_sectors_done < total_sectors {
            if self.write_buf_off < self.write_block_buf.len() {
                match cmd.read_data(&mut self.write_block_buf[self.write_buf_off..]) {
                    Ok(n) if n > 0 => self.write_buf_off += n,
                    _ => break,
                }
            }
            if self.write_buf_off != self.write_block_buf.len() {
                break;
            }

            let result = if Uf2Block::has_magic(&self.write_block_buf) {
                Uf2Block::parse(&self.write_block_buf)
                    .ok_or(UpdateError::MalformedUf2)
                    .and_then(|block| self.handle_uf2_block(&block))
            } else {
                // Filesystems also write FAT metadata to the virtual drive.
                Ok(())
            };
            if let Err(error) = result {
                self.reset_write_state();
                self.fail(cmd, error);
                return;
            }
            self.write_buf_off = 0;
            self.write_sectors_done += 1;
        }

        if self.write_sectors_done >= total_sectors {
            self.reset_write_state();
            cmd.pass();
        }
    }

    fn reset_write_state(&mut self) {
        self.write_buf_off = 0;
        self.write_sectors_done = 0;
        self.current_write_lba = None;
    }

    fn fail<'alloc, Bus: UsbBus + 'alloc, Buf: core::borrow::BorrowMut<[u8]>>(
        &mut self,
        cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>,
        error: UpdateError,
    ) {
        self.last_error = Some(error);
        cmd.fail();
    }

    pub fn handle_uf2_block(&mut self, block: &Uf2Block) -> Result<(), UpdateError> {
        let payload = block.payload();
        let observation = self
            .tracker
            .inspect(
                block.block_no,
                block.num_blocks,
                block.target_addr,
                payload.len(),
                FIRMWARE_START,
                FIRMWARE_END,
                UF2_PAYLOAD_SIZE,
            )
            .map_err(|error| match error {
                pager_bootloader_core::TrackError::InconsistentCount
                | pager_bootloader_core::TrackError::ConflictingLastPayload => {
                    UpdateError::InconsistentTransfer
                }
                _ => UpdateError::MalformedUf2,
            })?;

        if block.block_no == 0 {
            self.validate_manifest(payload, block.num_blocks)?;
        }

        if observation == Observation::Duplicate {
            let existing = unsafe {
                core::slice::from_raw_parts(block.target_addr as *const u8, payload.len())
            };
            return if existing == payload {
                Ok(())
            } else {
                Err(UpdateError::InconsistentTransfer)
            };
        }

        self.write_payload(block.target_addr, payload)?;
        self.tracker.commit(block.block_no);

        if self.tracker.complete() && self.manifest.is_some() {
            if !self.verify_flashed_image() {
                return Err(UpdateError::Verification);
            }
            self.reset_pending = true;
        }
        Ok(())
    }

    fn validate_manifest(&mut self, payload: &[u8], total_blocks: u32) -> Result<(), UpdateError> {
        if payload.len() != UF2_PAYLOAD_SIZE as usize
            || payload.len() < core::mem::size_of::<crate::manifest::Manifest>()
        {
            return Err(UpdateError::InvalidManifest);
        }
        let manifest = unsafe {
            core::ptr::read_unaligned(payload.as_ptr() as *const crate::manifest::Manifest)
        };
        if manifest.magic != crate::manifest::MAGIC || !image_len_is_valid(manifest.image_len) {
            return Err(UpdateError::InvalidManifest);
        }
        let full_len = MANIFEST_SIZE
            .checked_add(manifest.image_len)
            .ok_or(UpdateError::InvalidManifest)?;
        let expected_blocks = full_len.div_ceil(UF2_PAYLOAD_SIZE);
        let expected_last = full_len - (expected_blocks - 1) * UF2_PAYLOAD_SIZE;
        if total_blocks != expected_blocks
            || self
                .tracker
                .last_payload_len()
                .is_some_and(|len| len as u32 != expected_last)
        {
            return Err(UpdateError::InvalidManifest);
        }
        if !crate::public_key::verify_signature(&manifest.signed_message(), &manifest.signature) {
            return Err(UpdateError::InvalidSignature);
        }
        if self
            .manifest
            .is_some_and(|previous| previous.to_bytes() != manifest.to_bytes())
        {
            return Err(UpdateError::InconsistentTransfer);
        }
        self.manifest = Some(manifest);
        Ok(())
    }

    fn write_payload(&mut self, addr: u32, payload: &[u8]) -> Result<(), UpdateError> {
        let page_addr = addr & !(PAGE_SIZE - 1);
        let page_index = ((page_addr - FIRMWARE_START) / PAGE_SIZE) as usize;
        if page_index >= TOTAL_PAGES {
            return Err(UpdateError::MalformedUf2);
        }
        if !self.erased_pages[page_index] {
            self.nvmc
                .erase(page_addr, page_addr + PAGE_SIZE)
                .map_err(|_| UpdateError::Flash)?;
            self.erased_pages[page_index] = true;
        }
        self.nvmc
            .write(addr, payload)
            .map_err(|_| UpdateError::Flash)
    }

    fn verify_flashed_image(&self) -> bool {
        let manifest = unsafe {
            core::ptr::read_unaligned(FIRMWARE_START as *const crate::manifest::Manifest)
        };
        if self
            .manifest
            .is_none_or(|expected| expected.to_bytes() != manifest.to_bytes())
            || manifest.magic != crate::manifest::MAGIC
            || !image_len_is_valid(manifest.image_len)
            || !crate::public_key::verify_signature(&manifest.signed_message(), &manifest.signature)
        {
            return false;
        }
        let image_start = FIRMWARE_START + MANIFEST_SIZE;
        let image_end = match image_start.checked_add(manifest.image_len) {
            Some(end) if end <= FIRMWARE_END => end,
            _ => return false,
        };
        let image = unsafe {
            core::slice::from_raw_parts(
                image_start as *const u8,
                (image_end - image_start) as usize,
            )
        };
        Sha256::digest(image).as_slice() == manifest.digest
    }
}
