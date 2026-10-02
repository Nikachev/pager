#![no_std]

use sha2::{Digest, Sha256};

pub const PAGE_SIZE: u32 = 4096;
pub const BOOTLOADER_END: u32 = 0xC000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Identity,
    Bounds,
    Vector,
    Digest,
    Protected,
    Erase,
    Write,
    Verify,
}

/// Embedded in the signed application; the chip binding prevents installation
/// on another target even if the host sends the package to the wrong device.
pub struct Target {
    pub board: u32,
    pub serial: u64,
    pub bootloader_end: u32,
    pub digest: [u8; 32],
}

pub trait Flash {
    fn accessible(&self, end: u32) -> bool;
    fn erase_page(&mut self, address: u32) -> Result<(), Error>;
    fn write_word(&mut self, address: u32, value: u32) -> Result<(), Error>;
    fn read_word(&self, address: u32) -> u32;
    fn feed_watchdog(&mut self);
}

pub fn preflight(
    image: &[u8],
    target: &Target,
    board: u32,
    serial: u64,
    flash: &impl Flash,
) -> Result<(), Error> {
    if target.board != board || target.serial != serial {
        return Err(Error::Identity);
    }
    if target.bootloader_end != BOOTLOADER_END
        || image.len() < 8
        || image.len() > BOOTLOADER_END as usize
        || !image.len().is_multiple_of(4)
    {
        return Err(Error::Bounds);
    }
    pager_bootloader_core::codec::validate_vector(image, 0).map_err(|_| Error::Vector)?;
    if <[u8; 32]>::from(Sha256::digest(image)) != target.digest {
        return Err(Error::Digest);
    }
    if !flash.accessible(BOOTLOADER_END) {
        return Err(Error::Protected);
    }
    Ok(())
}

/// Only the bootloader partition is touched. The updater code, signed manifest
/// and user storage stay outside it. Callers must quiesce USB and interrupts.
pub fn replace(
    image: &[u8],
    target: &Target,
    board: u32,
    serial: u64,
    flash: &mut impl Flash,
) -> Result<(), Error> {
    preflight(image, target, board, serial, flash)?;
    // Write and verify the non-vector pages before committing the reset page.
    for page in (PAGE_SIZE..BOOTLOADER_END).step_by(PAGE_SIZE as usize) {
        replace_page(image, page, flash)?;
    }
    replace_page(image, 0, flash)?;
    let mut digest = Sha256::new();
    for address in (0..image.len() as u32).step_by(4) {
        flash.feed_watchdog();
        digest.update(flash.read_word(address).to_le_bytes());
    }
    if <[u8; 32]>::from(digest.finalize()) != target.digest {
        return Err(Error::Verify);
    }
    Ok(())
}

fn replace_page(image: &[u8], page: u32, flash: &mut impl Flash) -> Result<(), Error> {
    flash.feed_watchdog();
    flash.erase_page(page)?;
    // Commit the reset handler and initial stack after the rest of page zero.
    let first = if page == 0 { 8 } else { page };
    for address in (first..page + PAGE_SIZE).step_by(4) {
        program(image, address, flash)?;
    }
    if page == 0 {
        program(image, 4, flash)?;
        program(image, 0, flash)?;
    }
    for address in (page..page + PAGE_SIZE).step_by(4) {
        flash.feed_watchdog();
        if flash.read_word(address) != expected_word(image, address) {
            return Err(Error::Verify);
        }
    }
    Ok(())
}

fn expected_word(image: &[u8], address: u32) -> u32 {
    let start = address as usize;
    if start >= image.len() {
        u32::MAX
    } else {
        u32::from_le_bytes(image[start..start + 4].try_into().unwrap())
    }
}

fn program(image: &[u8], address: u32, flash: &mut impl Flash) -> Result<(), Error> {
    let word = expected_word(image, address);
    flash.feed_watchdog();
    if word != u32::MAX {
        flash.write_word(address, word)?;
    }
    Ok(())
}

pub fn acl_blocks(address: u32, size: u32, permissions: u32, end: u32) -> bool {
    size != 0 && permissions & 6 != 0 && address < end
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    struct Mock {
        memory: Vec<u32>,
        erased: Vec<u32>,
        writes: Vec<u32>,
        protected: bool,
        fail_erase: bool,
        fail_write: bool,
        corrupt: bool,
        feeds: usize,
    }
    impl Mock {
        fn new() -> Self {
            Self {
                memory: vec![0x12345678; 0x100000 / 4],
                erased: vec![],
                writes: vec![],
                protected: false,
                fail_erase: false,
                fail_write: false,
                corrupt: false,
                feeds: 0,
            }
        }
    }
    impl Flash for Mock {
        fn accessible(&self, _: u32) -> bool {
            !self.protected
        }
        fn erase_page(&mut self, a: u32) -> Result<(), Error> {
            if self.fail_erase {
                return Err(Error::Erase);
            }
            self.erased.push(a);
            self.memory[a as usize / 4..(a + PAGE_SIZE) as usize / 4].fill(u32::MAX);
            Ok(())
        }
        fn write_word(&mut self, a: u32, v: u32) -> Result<(), Error> {
            if self.fail_write {
                return Err(Error::Write);
            }
            self.writes.push(a);
            self.memory[a as usize / 4] = v;
            Ok(())
        }
        fn read_word(&self, a: u32) -> u32 {
            self.memory[a as usize / 4] ^ u32::from(self.corrupt)
        }
        fn feed_watchdog(&mut self) {
            self.feeds += 1;
        }
    }
    fn fixture() -> (Vec<u8>, Target) {
        let mut image = vec![0x55; 5000];
        image[..4].copy_from_slice(&0x20040000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x101u32.to_le_bytes());
        let target = Target {
            board: 1,
            serial: 42,
            bootloader_end: BOOTLOADER_END,
            digest: Sha256::digest(&image).into(),
        };
        (image, target)
    }
    #[test]
    fn commits_zero_last_preserves_application_and_storage_and_erases_tail() {
        let (image, target) = fixture();
        let mut flash = Mock::new();
        replace(&image, &target, 1, 42, &mut flash).unwrap();
        assert_eq!(flash.erased.len(), 12);
        assert_eq!(flash.erased.last(), Some(&0));
        assert_eq!(&flash.writes[flash.writes.len() - 2..], &[4, 0]);
        assert!(flash.memory[BOOTLOADER_END as usize / 4..]
            .iter()
            .all(|v| *v == 0x12345678));
        assert!(flash.memory[image.len() / 4..BOOTLOADER_END as usize / 4]
            .iter()
            .all(|v| *v == u32::MAX));
        assert!(flash.feeds > image.len() / 4);
    }
    #[test]
    fn all_preflight_failures_leave_flash_untouched() {
        let (mut image, mut target) = fixture();
        let mut flash = Mock::new();
        assert_eq!(
            replace(&image, &target, 2, 42, &mut flash),
            Err(Error::Identity)
        );
        assert_eq!(
            replace(&image, &target, 1, 43, &mut flash),
            Err(Error::Identity)
        );
        target.bootloader_end += PAGE_SIZE;
        assert_eq!(
            replace(&image, &target, 1, 42, &mut flash),
            Err(Error::Bounds)
        );
        target.bootloader_end = BOOTLOADER_END;
        image[4] &= !1;
        assert_eq!(
            replace(&image, &target, 1, 42, &mut flash),
            Err(Error::Vector)
        );
        image[4] |= 1;
        image[100] ^= 1;
        assert_eq!(
            replace(&image, &target, 1, 42, &mut flash),
            Err(Error::Digest)
        );
        image[100] ^= 1;
        flash.protected = true;
        assert_eq!(
            replace(&image, &target, 1, 42, &mut flash),
            Err(Error::Protected)
        );
        assert!(flash.erased.is_empty());
        assert!(flash.writes.is_empty());
    }
    #[test]
    fn hardware_failures_stop_before_page_zero_commit() {
        let (image, target) = fixture();
        for expected in [Error::Erase, Error::Write, Error::Verify] {
            let mut flash = Mock::new();
            flash.fail_erase = expected == Error::Erase;
            flash.fail_write = expected == Error::Write;
            flash.corrupt = expected == Error::Verify;
            assert_eq!(replace(&image, &target, 1, 42, &mut flash), Err(expected));
            assert!(!flash.erased.contains(&0));
            assert!(!flash.writes.contains(&0));
        }
    }
    #[test]
    fn acl_rejects_only_overlapping_denied_regions() {
        assert!(acl_blocks(0, 4096, 2, BOOTLOADER_END));
        assert!(acl_blocks(4096, 4096, 4, BOOTLOADER_END));
        assert!(!acl_blocks(0, 4096, 0, BOOTLOADER_END));
        assert!(!acl_blocks(0, 0, 6, BOOTLOADER_END));
        assert!(!acl_blocks(BOOTLOADER_END, 4096, 6, BOOTLOADER_END));
    }
}
