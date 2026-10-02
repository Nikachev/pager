//! Secure UF2 flashing engine for nRF52840 NVMC.
//!
//! Blocks may arrive out of order and may be repeated by an MSC host. Each flash
//! page is erased once, while a bitmap records unique UF2 blocks. Firmware is
//! never executed until the signed manifest and the complete image verify.

use crate::fat16::get_virtual_fat_sector;
use crate::memory_map::LAYOUT;
use crate::scsi::{handle_scsi_inquiry, handle_scsi_read_capacity};
use crate::uf2::Uf2Block;

use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::NorFlash;
pub use pager_bootloader_core::flash::Error as UpdateError;
use pager_bootloader_core::flash::{Engine, Flash, SignatureVerifier};
use usb_device::bus::UsbBus;
use usbd_storage::subclass::scsi::{Scsi, ScsiCommand};
use usbd_storage::transport::bbb::BulkOnly;

fn sense(error: UpdateError) -> (u8, u8, u8) {
    match error {
        UpdateError::MalformedUf2
        | UpdateError::InconsistentTransfer
        | UpdateError::InvalidManifest => (0x05, 0x24, 0),
        UpdateError::InvalidSignature | UpdateError::Verification => (0x03, 0x11, 0),
        UpdateError::Flash => (0x03, 0x0C, 2),
        UpdateError::UnsupportedCommand => (0x05, 0x20, 0),
    }
}

struct NvmcFlash<'a>(Nvmc<'a>);
impl Flash for NvmcFlash<'_> {
    fn read(&mut self, address: u32, bytes: &mut [u8]) -> Result<(), UpdateError> {
        if address < LAYOUT.firmware_start
            || address
                .checked_add(bytes.len() as u32)
                .is_none_or(|end| end > LAYOUT.storage_start)
        {
            return Err(UpdateError::Flash);
        }
        // Checked application bounds above cover the readable flash slice.
        let source = unsafe { core::slice::from_raw_parts(address as *const u8, bytes.len()) };
        bytes.copy_from_slice(source);
        Ok(())
    }
    fn erase(&mut self, start: u32, end: u32) -> Result<(), UpdateError> {
        if start < LAYOUT.firmware_start || end > LAYOUT.storage_start || end <= start {
            return Err(UpdateError::Flash);
        }
        self.0.erase(start, end).map_err(|_| UpdateError::Flash)
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), UpdateError> {
        if address < LAYOUT.firmware_start
            || address
                .checked_add(bytes.len() as u32)
                .is_none_or(|end| end > LAYOUT.storage_start)
        {
            return Err(UpdateError::Flash);
        }
        self.0.write(address, bytes).map_err(|_| UpdateError::Flash)
    }
}
struct Keys;
impl SignatureVerifier for Keys {
    fn verify(&self, message: &[u8; 48], signature: &[u8; 64]) -> bool {
        crate::public_key::verify_signature(message, signature)
    }
}

pub struct Uf2FlashEngine<'a> {
    flash: NvmcFlash<'a>,
    engine: Engine,
    write_block_buf: [u8; 512],
    write_buf_off: usize,
    write_sectors_done: usize,
    current_write_lba: Option<u64>,
    last_error: Option<UpdateError>,
    reset_pending: bool,
    read_cursor: pager_bootloader_core::transfer::SectorCursor,
    command_id: Option<u32>,
}

impl<'a> Uf2FlashEngine<'a> {
    pub fn new(nvmc: Nvmc<'a>) -> Self {
        Self {
            flash: NvmcFlash(nvmc),
            engine: Engine::new(LAYOUT).expect("validated firmware layout"),
            write_block_buf: [0; 512],
            write_buf_off: 0,
            write_sectors_done: 0,
            current_write_lba: None,
            last_error: None,
            reset_pending: false,
            read_cursor: Default::default(),
            command_id: None,
        }
    }

    pub fn take_reset_pending(&mut self) -> bool {
        core::mem::take(&mut self.reset_pending)
    }

    pub fn handle_scsi_command<'alloc, Bus: UsbBus + 'alloc, Buf: core::borrow::BorrowMut<[u8]>>(
        &mut self,
        mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>,
    ) {
        if self.command_id != Some(cmd.id()) {
            self.command_id = Some(cmd.id());
            self.read_cursor.reset();
            self.reset_write_state();
        }
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
                if lba.checked_add(len).is_none_or(|end| end > 131072) {
                    self.fail(cmd, UpdateError::MalformedUf2);
                    return;
                }
                self.read_cursor.begin(lba, len);
                let mut sector = [0u8; 512];
                while let Some((address, offset)) = self.read_cursor.position() {
                    let Ok(address) = u32::try_from(address) else {
                        self.read_cursor.reset();
                        self.fail(cmd, UpdateError::MalformedUf2);
                        return;
                    };
                    get_virtual_fat_sector(address, &mut sector);
                    match cmd.write_data(&sector[offset..]) {
                        Ok(n) if n > 0 => {
                            self.read_cursor.advance(n);
                        }
                        _ => return,
                    }
                }
                if !self.read_cursor.complete() {
                    self.read_cursor.reset();
                    self.fail(cmd, UpdateError::MalformedUf2);
                    return;
                }
                self.read_cursor.reset();
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
                    let (key, asc, ascq) = sense(error);
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
        if lba.checked_add(len).is_none_or(|end| end > 131072) {
            self.fail(cmd, UpdateError::MalformedUf2);
            return;
        }
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
                    .map_err(|_| UpdateError::MalformedUf2)
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
        // A failed transfer may have left partially programmed pages. A fresh
        // full package must erase them again rather than inherit its bitmap.
        self.engine = Engine::new(LAYOUT).expect("validated firmware layout");
        self.reset_pending = false;
        cmd.fail();
    }

    pub fn handle_uf2_block(&mut self, block: &Uf2Block) -> Result<(), UpdateError> {
        self.engine.accept(block, &mut self.flash, &Keys)?;
        self.reset_pending = self.engine.complete();
        Ok(())
    }
}
