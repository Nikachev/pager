use bt_hci::param::{AddrKind, BdAddr};
use embedded_storage_async::nor_flash::NorFlash;
use trouble_host::connection::SecurityLevel;
use trouble_host::prelude::{
    Address, BondInformation, Identity, IdentityResolvingKey, LongTermKey,
};

pub use pager::protocol::{crc32_finalize, crc32_update, CRC32_INIT};

/// Triggers a software reboot into DFU mode by writing the DFU magic flag
/// to GPREGRET (8-bit retention register at 0x4000_051C) and issuing an ARM system reset.
pub fn enter_bootloader() -> ! {
    const DFU_MAGIC: u32 = 0xB1; // 8-bit magic matching bootloader's DFU_MAGIC
    const NRF_POWER_GPREGRET: *mut u32 = 0x4000_051C as *mut u32;
    unsafe {
        core::ptr::write_volatile(NRF_POWER_GPREGRET, DFU_MAGIC);
    }
    cortex_m::asm::dsb();
    cortex_m::peripheral::SCB::sys_reset();
}

// Application settings and BLE bonds live in the 8 KiB STORAGE partition at 0xFE000
pub const STORAGE_START_ADDR: u32 = 0x000FE000;
pub const STORAGE_PAGE_SIZE: u32 = 4096;
pub const STORAGE_PAGE0: u32 = STORAGE_START_ADDR;
pub const STORAGE_PAGE1: u32 = STORAGE_START_ADDR + STORAGE_PAGE_SIZE;

const LEGACY_STORAGE_MAGIC: u32 = 0x50414745; // "PAGE"
const STORAGE_MAGIC: u32 = 0x3242_4750; // "PGB2"
const STORAGE_VERSION: u8 = 3;
const PREVIOUS_STORAGE_VERSION: u8 = 2;
const BOND_SLOT_COUNT: usize = 3;
const BOND_RECORD_LEN: usize = 43;
const STORAGE_HEADER_LEN: usize = 12;
const STORAGE_DATA_LEN: usize = STORAGE_HEADER_LEN + BOND_SLOT_COUNT * BOND_RECORD_LEN + 4;

/// Each record is 152 bytes (aligned to 4-byte boundary for Flash word writes).
/// A 4096-byte Flash page holds 26 slots of 152 bytes (26 * 152 = 3952 <= 4096).
pub const RECORD_SLOT_LEN: usize = STORAGE_DATA_LEN.div_ceil(4) * 4;
pub const SLOTS_PER_PAGE: usize = (STORAGE_PAGE_SIZE as usize) / RECORD_SLOT_LEN;

#[repr(align(4))]
struct AlignedRecord([u8; RECORD_SLOT_LEN]);
type PersistentState = (usize, [Option<BondInformation>; BOND_SLOT_COUNT]);
type StorageRecord = (u32, PersistentState);

/// Scans both STORAGE pages for the record with the highest sequence number.
pub async fn load_persistent_state<F: NorFlash>(
    flash: &mut F,
) -> Option<(usize, [Option<BondInformation>; BOND_SLOT_COUNT])> {
    let mut best_record: Option<StorageRecord> = None;

    for &page_addr in &[STORAGE_PAGE0, STORAGE_PAGE1] {
        for slot in 0..SLOTS_PER_PAGE {
            let addr = page_addr + (slot * RECORD_SLOT_LEN) as u32;
            let mut buf = [0u8; RECORD_SLOT_LEN];
            if flash.read(addr, &mut buf).await.is_err() {
                continue;
            }

            // Quick check for erased slot (all 0xFF)
            if buf.iter().all(|&b| b == 0xFF) {
                continue;
            }

            if let Some(record) = decode_storage(&buf).or_else(|| decode_storage_v2(&buf)) {
                best_record = select_storage(best_record, Some(record));
            } else {
                // Check legacy format at page start
                let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
                if magic == LEGACY_STORAGE_MAGIC && best_record.is_none() {
                    best_record = Some((
                        0,
                        (
                            (buf[4] as usize).min(BOND_SLOT_COUNT - 1),
                            [None, None, None],
                        ),
                    ));
                }
            }
        }
    }

    best_record.map(|(_, state)| state)
}

/// Appends a new state record into the next available Flash slot (Ring-Buffer Wear-Leveling).
/// Erases a 4 KB page ONLY when the current active page is completely filled (1:26 wear reduction).
pub async fn save_persistent_state<F: NorFlash>(
    flash: &mut F,
    active_profile: usize,
    bonds: &[Option<BondInformation>; BOND_SLOT_COUNT],
) -> Result<(), F::Error> {
    // 1. Scan current Flash state to find highest sequence number and last used slot
    let mut max_seq: u32 = 0;
    let mut max_page: u32 = STORAGE_PAGE0;
    let mut max_slot: Option<usize> = None;
    let mut found_any = false;

    for &page_addr in &[STORAGE_PAGE0, STORAGE_PAGE1] {
        for slot in 0..SLOTS_PER_PAGE {
            let addr = page_addr + (slot * RECORD_SLOT_LEN) as u32;
            let mut buf = [0u8; RECORD_SLOT_LEN];
            flash.read(addr, &mut buf).await?;

            if let Some((seq, _)) = decode_storage(&buf).or_else(|| decode_storage_v2(&buf)) {
                if !found_any || seq.wrapping_sub(max_seq) < 0x8000_0000 {
                    max_seq = seq;
                    max_page = page_addr;
                    max_slot = Some(slot);
                    found_any = true;
                }
            }
        }
    }

    let new_seq = if found_any {
        max_seq.wrapping_add(1)
    } else {
        1
    };

    // 2. Determine target write page & slot
    let (target_page, target_slot) = match (found_any, max_slot) {
        (true, Some(slot)) if slot + 1 < SLOTS_PER_PAGE => (max_page, slot + 1),
        (true, _) => {
            // Active page is full: switch to opposite page
            let next_page = if max_page == STORAGE_PAGE0 {
                STORAGE_PAGE1
            } else {
                STORAGE_PAGE0
            };
            (next_page, 0)
        }
        _ => (STORAGE_PAGE0, 0),
    };

    // If starting a new page (slot 0), erase target page first
    if target_slot == 0 {
        flash
            .erase(target_page, target_page + STORAGE_PAGE_SIZE)
            .await?;
    }

    // 3. Build and write the 152-byte record
    let mut storage = AlignedRecord([0xFFu8; RECORD_SLOT_LEN]);
    let buf = &mut storage.0;

    buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
    buf[4] = STORAGE_VERSION;
    buf[5] = active_profile.min(BOND_SLOT_COUNT - 1) as u8;
    buf[8..12].copy_from_slice(&new_seq.to_le_bytes());

    for (i, bond) in bonds.iter().enumerate() {
        let start = STORAGE_HEADER_LEN + i * BOND_RECORD_LEN;
        if let Some(bond) = bond {
            encode_bond(bond, &mut buf[start..start + BOND_RECORD_LEN]);
        }
    }

    let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
    buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());

    let target_addr = target_page + (target_slot * RECORD_SLOT_LEN) as u32;
    flash.write(target_addr, buf).await
}

fn decode_storage(buf: &[u8]) -> Option<StorageRecord> {
    if buf.len() < RECORD_SLOT_LEN
        || u32::from_le_bytes(buf[0..4].try_into().ok()?) != STORAGE_MAGIC
        || buf[4] != STORAGE_VERSION
    {
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
    let active_profile = (buf[5] as usize).min(BOND_SLOT_COUNT - 1);
    let mut bonds = [None, None, None];
    for (index, bond) in bonds.iter_mut().enumerate() {
        let start = STORAGE_HEADER_LEN + index * BOND_RECORD_LEN;
        *bond = decode_bond(&buf[start..start + BOND_RECORD_LEN]);
    }
    Some((
        u32::from_le_bytes(buf[8..12].try_into().ok()?),
        (active_profile, bonds),
    ))
}

fn decode_storage_v2(buf: &[u8]) -> Option<StorageRecord> {
    const V2_HEADER_LEN: usize = 8;
    const V2_DATA_LEN: usize = V2_HEADER_LEN + BOND_SLOT_COUNT * BOND_RECORD_LEN + 4;
    if buf.len() < V2_DATA_LEN
        || u32::from_le_bytes(buf[0..4].try_into().ok()?) != STORAGE_MAGIC
        || buf[4] != PREVIOUS_STORAGE_VERSION
    {
        return None;
    }
    let stored_crc = u32::from_le_bytes(buf[V2_DATA_LEN - 4..V2_DATA_LEN].try_into().ok()?);
    if crc32_finalize(crc32_update(CRC32_INIT, &buf[..V2_DATA_LEN - 4])) != stored_crc {
        return None;
    }
    let active_profile = (buf[5] as usize).min(BOND_SLOT_COUNT - 1);
    let mut bonds = [None, None, None];
    for (index, bond) in bonds.iter_mut().enumerate() {
        let start = V2_HEADER_LEN + index * BOND_RECORD_LEN;
        *bond = decode_bond(&buf[start..start + BOND_RECORD_LEN]);
    }
    Some((0, (active_profile, bonds)))
}

fn select_storage(
    first: Option<StorageRecord>,
    second: Option<StorageRecord>,
) -> Option<StorageRecord> {
    match (first, second) {
        (Some(a), Some(b)) if b.0.wrapping_sub(a.0) < 0x8000_0000 => Some(b),
        (Some(a), _) => Some(a),
        (_, Some(b)) => Some(b),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_crc_and_decode_roundtrip() {
        let mut buf = [0xFFu8; RECORD_SLOT_LEN];
        buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
        buf[4] = STORAGE_VERSION;
        buf[5] = 1; // active_slot
        buf[8..12].copy_from_slice(&105u32.to_le_bytes()); // seq

        let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
        buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());

        let decoded = decode_storage(&buf);
        assert!(decoded.is_some());
        let (seq, (slot, bonds)) = decoded.unwrap();
        assert_eq!(seq, 105);
        assert_eq!(slot, 1);
        assert_eq!(bonds, [None, None, None]);
    }

    #[test]
    fn test_select_storage_sequence_wrap() {
        let rec1 = Some((100u32, (0, [None, None, None])));
        let rec2 = Some((101u32, (1, [None, None, None])));
        assert_eq!(select_storage(rec1, rec2).unwrap().0, 101);

        let rec_wrapped = Some((1u32, (0, [None, None, None])));
        let rec_old = Some((0xFFFF_FFFFu32, (1, [None, None, None])));
        assert_eq!(select_storage(rec_old, rec_wrapped).unwrap().0, 1);
    }
}
