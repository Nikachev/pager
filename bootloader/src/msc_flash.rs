//! Virtual FAT12 / SCSI USB Mass Storage Flashing Engine for UF2

use crate::uf2::Uf2Block;
use ed25519_dalek::{Signature, VerifyingKey};
use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::NorFlash;
use sha2::{Digest, Sha256};

pub const FIRMWARE_START: u32 = 0x0000_C000;
pub const FIRMWARE_END: u32 = 0x000F_E000;
pub const PAGE_SIZE: u32 = 4096;
const TOTAL_PAGES: usize = ((FIRMWARE_END - FIRMWARE_START) / PAGE_SIZE) as usize;

pub struct Uf2FlashEngine<'a> {
    nvmc: Nvmc<'a>,
    erased_pages: [bool; TOTAL_PAGES],
    hasher: Sha256,
    block0_verified: bool,
    expected_digest: [u8; 32],
    blocks_received: u32,
    total_blocks: u32,
}

impl<'a> Uf2FlashEngine<'a> {
    pub fn new(nvmc: Nvmc<'a>) -> Self {
        Self {
            nvmc,
            erased_pages: [false; TOTAL_PAGES],
            hasher: Sha256::new(),
            block0_verified: false,
            expected_digest: [0; 32],
            blocks_received: 0,
            total_blocks: 0,
        }
    }

    pub fn handle_uf2_block(&mut self, block: &Uf2Block) -> Result<(), ()> {
        let addr = block.target_addr;
        let payload = block.payload();

        // 1. Check Flash Boundary Protection
        if addr < FIRMWARE_START || addr + (payload.len() as u32) > FIRMWARE_END {
            return Ok(()); // Ignore blocks outside application firmware slot
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
            let valid_sig = crate::public_key::FIRMWARE_SIGNING_PUBLIC_KEYS
                .iter()
                .any(|key_bytes| {
                    VerifyingKey::from_bytes(key_bytes).is_ok_and(|key| {
                        key.verify_strict(&signed_msg, &Signature::from_bytes(&manifest.signature))
                            .is_ok()
                    })
                });

            if !valid_sig {
                return Err(());
            }

            self.block0_verified = true;
            self.expected_digest = manifest.digest;
            self.total_blocks = block.num_blocks;

            // Instantly invalidate existing firmware marker in Flash page 0 before write
            self.erase_page_if_needed(FIRMWARE_START)?;
        }

        if !self.block0_verified {
            return Ok(()); // Hold writing until Block 0 verified
        }

        // 3. Page Erase Tracker (4 KB)
        self.erase_page_if_needed(addr)?;

        // 4. Write 256B Payload to Flash
        self.nvmc.write(addr, payload).map_err(|_| ())?;

        // 5. Update Sha256 Context (only for raw firmware payload, block_no > 0)
        if block.block_no > 0 {
            self.hasher.update(payload);
        }
        self.blocks_received += 1;

        // 6. Check Completion
        if self.blocks_received >= self.total_blocks && self.total_blocks > 0 {
            cortex_m::peripheral::SCB::sys_reset();
        }

        Ok(())
    }

    fn erase_page_if_needed(&mut self, addr: u32) -> Result<(), ()> {
        let page_index = ((addr - FIRMWARE_START) / PAGE_SIZE) as usize;
        if page_index < TOTAL_PAGES && !self.erased_pages[page_index] {
            let page_addr = FIRMWARE_START + (page_index as u32 * PAGE_SIZE);
            self.nvmc
                .erase(page_addr, page_addr + PAGE_SIZE)
                .map_err(|_| ())?;
            self.erased_pages[page_index] = true;
        }
        Ok(())
    }
}
