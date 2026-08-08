//! High-Performance UF2 Flashing Engine with 4KB Page Buffering for nRF52840 NVMC

use crate::fat16::get_virtual_fat_sector;
use crate::memory_map::{
    is_within_firmware_slot, FIRMWARE_END, FIRMWARE_START, PAGE_SIZE, TOTAL_PAGES,
};
use crate::scsi::{handle_scsi_inquiry, handle_scsi_read_capacity};
use crate::uf2::Uf2Block;

use ed25519_dalek::{Signature, VerifyingKey};
use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::NorFlash;
use sha2::{Digest, Sha256};

use usb_device::bus::UsbBus;
use usbd_storage::subclass::scsi::{Scsi, ScsiCommand};
use usbd_storage::transport::bbb::BulkOnly;

pub struct Uf2FlashEngine<'a> {
    nvmc: Nvmc<'a>,
    erased_pages: [bool; TOTAL_PAGES],
    block0_verified: bool,
    blocks_received: u32,
    total_blocks: u32,
    write_block_buf: [u8; 512],
    write_buf_off: usize,
    write_sectors_done: usize,
    current_write_lba: Option<u64>,

    // High-performance 4KB Page Buffering (Reduces NVMC CONFIG.WEN toggles by 16x)
    page_buf: [u8; PAGE_SIZE as usize],
    buffered_page_addr: Option<u32>,
    page_dirty: bool,
}

impl<'a> Uf2FlashEngine<'a> {
    pub fn new(nvmc: Nvmc<'a>) -> Self {
        Self {
            nvmc,
            erased_pages: [false; TOTAL_PAGES],
            block0_verified: false,
            blocks_received: 0,
            total_blocks: 0,
            write_block_buf: [0u8; 512],
            write_buf_off: 0,
            write_sectors_done: 0,
            current_write_lba: None,
            page_buf: [0xFF; PAGE_SIZE as usize],
            buffered_page_addr: None,
            page_dirty: false,
        }
    }

    pub fn handle_scsi_command<'alloc, Bus: UsbBus + 'alloc, Buf: core::borrow::BorrowMut<[u8]>>(
        &mut self,
        mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>,
    ) {
        match cmd.kind {
            ScsiCommand::TestUnitReady => {
                cmd.pass();
            }
            ScsiCommand::Inquiry { alloc_len, .. } => {
                handle_scsi_inquiry(cmd, alloc_len);
            }
            ScsiCommand::ReadCapacity10 => {
                handle_scsi_read_capacity(cmd);
            }
            ScsiCommand::ReadCapacity16 { alloc_len } => {
                let mut resp = [0u8; 32];
                resp[0..8].copy_from_slice(&131071u64.to_be_bytes());
                resp[8..12].copy_from_slice(&512u32.to_be_bytes());
                let send_len = (alloc_len as usize).min(32);
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::ReadFormatCapacities { alloc_len } => {
                let mut resp = [0u8; 12];
                resp[3] = 8;
                resp[4..8].copy_from_slice(&131072u32.to_be_bytes());
                resp[8] = 0x02; // Formatted media
                resp[9..12].copy_from_slice(&[0x00, 0x02, 0x00]); // 512 bytes block size
                let send_len = (alloc_len as usize).min(12);
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::Read { lba, len } => {
                let total_sectors = len as usize;
                let mut sector_buf = [0u8; 512];
                for s in 0..total_sectors {
                    let cur_lba = (lba as u32) + (s as u32);
                    get_virtual_fat_sector(cur_lba, &mut sector_buf);
                    let mut written = 0;
                    while written < 512 {
                        match cmd.write_data(&sector_buf[written..]) {
                            Ok(n) if n > 0 => written += n,
                            _ => break,
                        }
                    }
                    if written < 512 {
                        return;
                    }
                }
                cmd.pass();
            }
            ScsiCommand::Write { lba, len } => {
                let total_sectors = len as usize;
                if self.current_write_lba != Some(lba) {
                    self.current_write_lba = Some(lba);
                    self.write_buf_off = 0;
                    self.write_sectors_done = 0;
                }

                while self.write_sectors_done < total_sectors {
                    if self.write_buf_off < 512 {
                        match cmd.read_data(&mut self.write_block_buf[self.write_buf_off..]) {
                            Ok(n) if n > 0 => self.write_buf_off += n,
                            _ => break,
                        }
                    }
                    if self.write_buf_off >= 512 {
                        if let Some(uf2) = Uf2Block::parse(&self.write_block_buf) {
                            let _ = self.handle_uf2_block(&uf2);
                        }
                        self.write_buf_off = 0;
                        self.write_sectors_done += 1;
                    } else {
                        break;
                    }
                }

                if self.write_sectors_done >= total_sectors {
                    self.write_buf_off = 0;
                    self.write_sectors_done = 0;
                    self.current_write_lba = None;
                    cmd.pass();
                }
            }
            ScsiCommand::ModeSense6 { alloc_len, .. } => {
                let resp = [0x03, 0x00, 0x00, 0x00];
                let send_len = (alloc_len as usize).min(4);
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::ModeSense10 { alloc_len, .. } => {
                let resp = [0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                let send_len = (alloc_len as usize).min(8);
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::RequestSense { alloc_len, .. } => {
                let mut resp = [0u8; 18];
                resp[0] = 0x70; // Current sense data
                resp[2] = 0x00; // Sense Key: NO SENSE
                resp[7] = 10; // Additional length
                resp[12] = 0x00; // Additional Sense Code: No Additional Sense
                let send_len = (alloc_len as usize).min(18);
                let _ = cmd.write_data(&resp[..send_len]);
                cmd.pass();
            }
            ScsiCommand::Unknown => {
                cmd.pass();
            }
        }
    }

    pub fn handle_uf2_block(&mut self, block: &Uf2Block) -> Result<(), ()> {
        let addr = block.target_addr;
        let payload = block.payload();

        // 1. Check Address Alignment & Flash Boundary Protection
        if !addr.is_multiple_of(256) || !is_within_firmware_slot(addr, payload.len() as u32) {
            return Ok(()); // Ignore invalid/unaligned blocks
        }

        // 2. Block 0 processing: Manifest & Signature validation in RAM
        if block.block_no == 0 {
            if payload.len() < core::mem::size_of::<crate::manifest::Manifest>() {
                return Err(());
            }
            let manifest = unsafe {
                core::ptr::read_unaligned(payload.as_ptr() as *const crate::manifest::Manifest)
            };

            if manifest.magic != crate::manifest::MAGIC {
                return Err(());
            }

            // Verify Ed25519 signature
            let signed_msg = manifest.signed_message();
            let valid_sig =
                crate::public_key::FIRMWARE_SIGNING_PUBLIC_KEYS
                    .iter()
                    .any(|key_bytes| {
                        VerifyingKey::from_bytes(key_bytes).is_ok_and(|key| {
                            key.verify_strict(
                                &signed_msg,
                                &Signature::from_bytes(&manifest.signature),
                            )
                            .is_ok()
                        })
                    });

            if !valid_sig {
                return Err(());
            }

            self.block0_verified = true;
            self.total_blocks = block.num_blocks;

            // Instantly invalidate existing firmware marker in Flash page 0 before write
            self.erase_page_direct(FIRMWARE_START)?;
        }

        if !self.block0_verified {
            return Ok(()); // Hold writing until Block 0 verified
        }

        // 3. 4KB Page-at-a-time Flash Buffering
        let page_addr = addr & !(PAGE_SIZE - 1);
        if self.buffered_page_addr != Some(page_addr) {
            self.flush_buffered_page()?;
            self.buffered_page_addr = Some(page_addr);
            self.page_buf.fill(0xFF);
        }

        let offset_in_page = (addr - page_addr) as usize;
        if offset_in_page + payload.len() <= PAGE_SIZE as usize {
            self.page_buf[offset_in_page..offset_in_page + payload.len()].copy_from_slice(payload);
            self.page_dirty = true;
        }

        self.blocks_received += 1;

        // 4. Check Completion & Verify SHA256 Digest from NVMC Flash
        if self.blocks_received >= self.total_blocks && self.total_blocks > 0 {
            self.flush_buffered_page()?;
            if self.verify_flashed_image() {
                cortex_m::asm::delay(50 * 64_000); // 50ms delay for CSW to flush over USB
                cortex_m::peripheral::SCB::sys_reset();
            }
        }

        Ok(())
    }

    fn flush_buffered_page(&mut self) -> Result<(), ()> {
        if self.page_dirty {
            if let Some(page_addr) = self.buffered_page_addr {
                self.erase_page_direct(page_addr)?;
                self.nvmc.write(page_addr, &self.page_buf).map_err(|_| ())?;
                self.page_dirty = false;
            }
        }
        Ok(())
    }

    fn erase_page_direct(&mut self, page_addr: u32) -> Result<(), ()> {
        let page_index = ((page_addr - FIRMWARE_START) / PAGE_SIZE) as usize;
        if page_index < TOTAL_PAGES && !self.erased_pages[page_index] {
            self.nvmc
                .erase(page_addr, page_addr + PAGE_SIZE)
                .map_err(|_| ())?;
            self.erased_pages[page_index] = true;
        }
        Ok(())
    }

    fn verify_flashed_image(&self) -> bool {
        let start_ptr = FIRMWARE_START as *const u8;
        let manifest_ptr = start_ptr as *const crate::manifest::Manifest;
        let manifest = unsafe { core::ptr::read_unaligned(manifest_ptr) };

        if manifest.magic != crate::manifest::MAGIC || manifest.image_len == 0 {
            return false;
        }

        let image_len = manifest.image_len as usize;
        let max_len = (FIRMWARE_END - FIRMWARE_START) as usize;
        if image_len > max_len {
            return false;
        }

        let image_start = FIRMWARE_START + crate::memory_map::MANIFEST_SIZE;
        let image_slice =
            unsafe { core::slice::from_raw_parts(image_start as *const u8, image_len) };
        let computed_digest = Sha256::digest(image_slice);

        computed_digest.as_slice() == manifest.digest
    }
}
