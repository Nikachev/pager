use core::sync::atomic::{AtomicBool, Ordering};

pub static IS_FLASHING: AtomicBool = AtomicBool::new(false);

pub fn try_start_flashing() -> bool {
    IS_FLASHING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn stop_flashing() {
    IS_FLASHING.store(false, Ordering::SeqCst);
}

pub fn reset_to_bootloader() -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

/// IEEE CRC-32 used by both OTA transports to detect a corrupted staged image.
/// The caller starts with [`CRC32_INIT`] and finalizes with [`crc32_finalize`].
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;

pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub const fn crc32_finalize(crc: u32) -> u32 {
    !crc
}

use crate::web::{inactive_slot_manifest_addr, MAX_BIN_SIZE};
use bt_hci::param::{AddrKind, BdAddr};
use embedded_storage_async::nor_flash::NorFlash;
use trouble_host::connection::SecurityLevel;
use trouble_host::prelude::{
    Address, BondInformation, Identity, IdentityResolvingKey, LongTermKey,
};

#[allow(dead_code)]
pub async fn erase_staging_area<F: NorFlash>(flash: &mut F) -> Result<(), F::Error> {
    erase_staging_range(flash, inactive_slot_manifest_addr(), MAX_BIN_SIZE as u32).await
}

pub async fn erase_staging_range<F: NorFlash>(
    flash: &mut F,
    start_addr: u32,
    len_bytes: u32,
) -> Result<(), F::Error> {
    let page_size = 4096u32;
    let num_pages = len_bytes.div_ceil(page_size);
    for page_idx in 0..num_pages {
        let page_addr = start_addr + page_idx * page_size;
        flash.erase(page_addr, page_addr + page_size).await?;
    }
    Ok(())
}

/// Cheap pre-reboot guard for OTA transports. The bootloader remains the
/// cryptographic authority, but this prevents an operator from successfully
/// uploading a valid package addressed to the currently running slot.
pub async fn package_targets_slot<F: NorFlash>(
    flash: &mut F,
    manifest_addr: u32,
    expected_slot: u32,
) -> Result<bool, F::Error> {
    let mut header = [0u8; 24];
    flash.read(manifest_addr, &mut header).await?;
    Ok(
        u32::from_le_bytes(header[0..4].try_into().unwrap()) == u32::MAX
            && header[4..12] == *b"PGRFW001"
            && u32::from_le_bytes(header[20..24].try_into().unwrap()) == expected_slot,
    )
}

pub struct OtaWriter<'a, F: NorFlash> {
    flash: &'a mut F,
    offset: u32,
    last_erased_page: u32,
    write_buffer: [u8; 512],
    write_buf_len: usize,
}

impl<'a, F: NorFlash> OtaWriter<'a, F> {
    pub fn new(flash: &'a mut F, start_addr: u32) -> Self {
        Self {
            flash,
            offset: start_addr,
            last_erased_page: u32::MAX,
            write_buffer: [0u8; 512],
            write_buf_len: 0,
        }
    }

    #[allow(dead_code)]
    pub fn get_offset(&self) -> u32 {
        self.offset
    }

    pub async fn write_chunk(&mut self, data: &[u8]) -> Result<(), F::Error> {
        let page_size = 4096u32;
        let mut data_idx = 0;
        while data_idx < data.len() {
            let page_addr = self.offset & !(page_size - 1);
            if page_addr != self.last_erased_page {
                self.flash.erase(page_addr, page_addr + page_size).await?;
                self.last_erased_page = page_addr;
                embassy_time::Timer::after(embassy_time::Duration::from_millis(5)).await;
            }

            let chunk_size = core::cmp::min(
                data.len() - data_idx,
                self.write_buffer.len() - self.write_buf_len,
            );
            self.write_buffer[self.write_buf_len..self.write_buf_len + chunk_size]
                .copy_from_slice(&data[data_idx..data_idx + chunk_size]);
            self.write_buf_len += chunk_size;
            data_idx += chunk_size;

            if self.write_buf_len == self.write_buffer.len() {
                self.flash.write(self.offset, &self.write_buffer).await?;
                self.offset += self.write_buffer.len() as u32;
                self.write_buf_len = 0;
            }
        }
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), F::Error> {
        if self.write_buf_len > 0 {
            while !self.write_buf_len.is_multiple_of(4) {
                self.write_buffer[self.write_buf_len] = 0xFF;
                self.write_buf_len += 1;
            }
            let page_size = 4096u32;
            let page_addr = self.offset & !(page_size - 1);
            if page_addr != self.last_erased_page {
                self.flash.erase(page_addr, page_addr + page_size).await?;
                self.last_erased_page = page_addr;
            }
            self.flash
                .write(self.offset, &self.write_buffer[..self.write_buf_len])
                .await?;
            self.offset += self.write_buf_len as u32;
            self.write_buf_len = 0;
        }
        Ok(())
    }
}

// Two pages at 0xfc000..0xfe000 form the power-loss-safe A/B boot-control
// journal. Application settings and BLE bonds live in the remaining 8 KiB.
pub const STORAGE_START_ADDR: u32 = 0x000FE000;
/// Version 1 stored only booleans and could not restore encrypted BLE links.
const LEGACY_STORAGE_MAGIC: u32 = 0x50414745; // "PAGE"
const STORAGE_MAGIC: u32 = 0x3242_4750; // "PGB2"
const STORAGE_VERSION: u8 = 2;
const BOND_SLOT_COUNT: usize = 3;
const BOND_RECORD_LEN: usize = 43;
const STORAGE_HEADER_LEN: usize = 8;
const STORAGE_DATA_LEN: usize = STORAGE_HEADER_LEN + BOND_SLOT_COUNT * BOND_RECORD_LEN + 4;
// NOR flash writes must use a length divisible by a word. The final three
// erased bytes are padding and deliberately excluded from the CRC.
const STORAGE_LEN: usize = STORAGE_DATA_LEN.div_ceil(4) * 4;

// NVMC requires the source pointer passed to `write` to be word-aligned.
// A `[u8; N]` local has alignment 1 even when its length is a multiple of 4.
#[repr(align(4))]
struct AlignedStorage([u8; STORAGE_LEN]);
const BOOT_CONTROL_PAGE0: u32 = 0x000F_C000;
const BOOT_CONTROL_PAGE1: u32 = 0x000F_D000;
const BOOT_CONTROL_MAGIC: [u8; 8] = *b"PGRAB001";
const NO_TRIAL_SLOT: u32 = u32::MAX;
const BOOT_CONTROL_LEN: usize = 32;

#[derive(Clone, Copy)]
struct BootControl {
    generation: u32,
    confirmed_slot: u32,
    confirmed_version: u32,
    trial_slot: u32,
    trial_version: u32,
}

/// The confirmed version is diagnostic only; package signatures and the
/// bootloader remain the authority for which image may execute.
pub fn installed_version() -> u32 {
    read_boot_control_raw().map_or(0, |(_, control)| control.confirmed_version)
}

/// Mark this trial image permanent only after the main runtime initialized.
/// A reset before this write leaves the prior journal record valid, so the
/// bootloader rolls back without erasing either application slot.
pub async fn confirm_running_slot<F: NorFlash>(flash: &mut F, slot: u32) -> Result<bool, F::Error> {
    let Some((current_page, control)) = read_boot_control(flash).await? else {
        return Ok(false);
    };
    if control.trial_slot != slot {
        return Ok(false);
    }
    let next = BootControl {
        generation: control.generation.wrapping_add(1),
        confirmed_slot: slot,
        confirmed_version: control.trial_version,
        trial_slot: NO_TRIAL_SLOT,
        trial_version: 0,
    };
    let target_page = if current_page == BOOT_CONTROL_PAGE0 {
        BOOT_CONTROL_PAGE1
    } else {
        BOOT_CONTROL_PAGE0
    };
    let record = encode_boot_control(next);
    flash.erase(target_page, target_page + 4096).await?;
    flash.write(target_page, &record).await?;
    Ok(true)
}

async fn read_boot_control<F: NorFlash>(
    flash: &mut F,
) -> Result<Option<(u32, BootControl)>, F::Error> {
    let mut first = [0u8; BOOT_CONTROL_LEN];
    let mut second = [0u8; BOOT_CONTROL_LEN];
    flash.read(BOOT_CONTROL_PAGE0, &mut first).await?;
    flash.read(BOOT_CONTROL_PAGE1, &mut second).await?;
    Ok(select_boot_control(
        decode_boot_control(&first),
        decode_boot_control(&second),
    ))
}

fn read_boot_control_raw() -> Option<(u32, BootControl)> {
    let first =
        unsafe { core::slice::from_raw_parts(BOOT_CONTROL_PAGE0 as *const u8, BOOT_CONTROL_LEN) };
    let second =
        unsafe { core::slice::from_raw_parts(BOOT_CONTROL_PAGE1 as *const u8, BOOT_CONTROL_LEN) };
    select_boot_control(decode_boot_control(first), decode_boot_control(second))
}

fn select_boot_control(
    first: Option<BootControl>,
    second: Option<BootControl>,
) -> Option<(u32, BootControl)> {
    match (first, second) {
        (Some(a), Some(b)) if b.generation.wrapping_sub(a.generation) < 0x8000_0000 => {
            Some((BOOT_CONTROL_PAGE1, b))
        }
        (Some(a), _) => Some((BOOT_CONTROL_PAGE0, a)),
        (_, Some(b)) => Some((BOOT_CONTROL_PAGE1, b)),
        _ => None,
    }
}

fn decode_boot_control(bytes: &[u8]) -> Option<BootControl> {
    if bytes.len() != BOOT_CONTROL_LEN || bytes[..8] != BOOT_CONTROL_MAGIC {
        return None;
    }
    let stored = u32::from_le_bytes(bytes[28..32].try_into().ok()?);
    if boot_control_checksum(&bytes[..28]) != stored {
        return None;
    }
    let control = BootControl {
        generation: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
        confirmed_slot: u32::from_le_bytes(bytes[12..16].try_into().ok()?),
        confirmed_version: u32::from_le_bytes(bytes[16..20].try_into().ok()?),
        trial_slot: u32::from_le_bytes(bytes[20..24].try_into().ok()?),
        trial_version: u32::from_le_bytes(bytes[24..28].try_into().ok()?),
    };
    (control.confirmed_slot <= 1
        && (control.trial_slot <= 1 || control.trial_slot == NO_TRIAL_SLOT))
        .then_some(control)
}

fn encode_boot_control(control: BootControl) -> [u8; BOOT_CONTROL_LEN] {
    let mut bytes = [0xFF; BOOT_CONTROL_LEN];
    bytes[..8].copy_from_slice(&BOOT_CONTROL_MAGIC);
    bytes[8..12].copy_from_slice(&control.generation.to_le_bytes());
    bytes[12..16].copy_from_slice(&control.confirmed_slot.to_le_bytes());
    bytes[16..20].copy_from_slice(&control.confirmed_version.to_le_bytes());
    bytes[20..24].copy_from_slice(&control.trial_slot.to_le_bytes());
    bytes[24..28].copy_from_slice(&control.trial_version.to_le_bytes());
    let checksum = boot_control_checksum(&bytes[..28]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

fn boot_control_checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811C_9DC5u32, |value, byte| {
        (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

pub async fn load_persistent_state<F: NorFlash>(
    flash: &mut F,
) -> Option<(usize, [Option<BondInformation>; BOND_SLOT_COUNT])> {
    let mut buf = [0u8; STORAGE_LEN];
    if flash.read(STORAGE_START_ADDR, &mut buf).await.is_err() {
        return None;
    }
    let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    if magic == LEGACY_STORAGE_MAGIC {
        // The pre-v2 layout never held key material.  Retain the selected
        // slot, but deliberately discard its boolean "bonded" markers: they
        // cannot establish encryption after reboot.
        return Some((
            (buf[4] as usize).min(BOND_SLOT_COUNT - 1),
            [None, None, None],
        ));
    }
    if magic != STORAGE_MAGIC || buf[4] != STORAGE_VERSION {
        return None;
    }
    let stored_crc = u32::from_le_bytes(
        buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN]
            .try_into()
            .ok()?,
    );
    if crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4])) != stored_crc {
        return None;
    }
    let active_slot = (buf[5] as usize).min(BOND_SLOT_COUNT - 1);
    let mut bonds = [None, None, None];
    for (index, bond) in bonds.iter_mut().enumerate() {
        let start = STORAGE_HEADER_LEN + index * BOND_RECORD_LEN;
        *bond = decode_bond(&buf[start..start + BOND_RECORD_LEN]);
    }
    Some((active_slot, bonds))
}

pub async fn save_persistent_state<F: NorFlash>(
    flash: &mut F,
    active_slot: usize,
    bonds: &[Option<BondInformation>; BOND_SLOT_COUNT],
) -> Result<(), F::Error> {
    flash
        .erase(STORAGE_START_ADDR, STORAGE_START_ADDR + 4096)
        .await?;
    let mut storage = AlignedStorage([0xFFu8; STORAGE_LEN]);
    let buf = &mut storage.0;
    buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
    buf[4] = STORAGE_VERSION;
    buf[5] = active_slot.min(BOND_SLOT_COUNT - 1) as u8;
    for (i, bond) in bonds.iter().enumerate() {
        let start = STORAGE_HEADER_LEN + i * BOND_RECORD_LEN;
        if let Some(bond) = bond {
            encode_bond(bond, &mut buf[start..start + BOND_RECORD_LEN]);
        }
    }
    let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
    buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());
    flash.write(STORAGE_START_ADDR, buf).await
}

fn encode_bond(bond: &BondInformation, out: &mut [u8]) {
    out.fill(0xFF);
    out[0] = 1;
    out[1] = bond.identity.addr.kind.as_raw();
    out[2..8].copy_from_slice(bond.identity.addr.addr.raw());
    if let Some(irk) = bond.identity.irk {
        out[8] = 1;
        out[9..25].copy_from_slice(&irk.to_le_bytes());
    }
    out[25..41].copy_from_slice(&bond.ltk.to_le_bytes());
    out[41] = match bond.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };
    out[42] = u8::from(bond.is_bonded);
}

fn decode_bond(data: &[u8]) -> Option<BondInformation> {
    if data.len() != BOND_RECORD_LEN || data[0] != 1 {
        return None;
    }
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&data[2..8]);
    let irk = match data[8] {
        0 => None,
        1 => {
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&data[9..25]);
            Some(IdentityResolvingKey::from_le_bytes(bytes)?)
        }
        _ => return None,
    };
    let mut ltk = [0u8; 16];
    ltk.copy_from_slice(&data[25..41]);
    let security_level = match data[41] {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };
    Some(BondInformation {
        ltk: LongTermKey::from_le_bytes(ltk),
        identity: Identity {
            addr: Address::new(AddrKind::new(data[1]), BdAddr::new(addr)),
            irk,
        },
        is_bonded: data[42] != 0,
        security_level,
    })
}
