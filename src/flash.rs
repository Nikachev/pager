use bt_hci::param::{AddrKind, BdAddr};
use embedded_storage_async::nor_flash::NorFlash;
use trouble_host::connection::SecurityLevel;
use trouble_host::prelude::{
    Address, BondInformation, Identity, IdentityResolvingKey, LongTermKey,
};

pub use crate::protocol::{crc32_finalize, crc32_update, CRC32_INIT};

/// Triggers a software reboot into DFU mode by writing the DFU magic flag
/// to GPREGRET (8-bit retention register at 0x4000_051C) and issuing an ARM system reset.
#[cfg(target_arch = "arm")]
pub fn enter_bootloader() -> ! {
    const DFU_MAGIC: u32 = 0xB1; // 8-bit magic matching bootloader's DFU_MAGIC
    const NRF_POWER_GPREGRET: *mut u32 = 0x4000_051C as *mut u32;
    unsafe {
        core::ptr::write_volatile(NRF_POWER_GPREGRET, DFU_MAGIC);
    }
    cortex_m::asm::dsb();
    cortex_m::peripheral::SCB::sys_reset();
}

// Application settings and BLE bonds live in the layout-defined storage partition.
pub const STORAGE_START_ADDR: u32 = crate::layout::STORAGE_START;
pub const STORAGE_PAGE_SIZE: u32 = crate::layout::FLASH_PAGE_SIZE;
pub const STORAGE_PAGE0: u32 = STORAGE_START_ADDR;
pub const STORAGE_PAGE1: u32 = STORAGE_START_ADDR + STORAGE_PAGE_SIZE;

const STORAGE_MAGIC: u32 = 0x3753_4750; // "PGS7"
const STORAGE_VERSION: u8 = 7;
const STORAGE_COMMIT: u32 = 0x434F_4D54; // "TMOC" in little-endian storage
const BOND_SLOT_COUNT: usize = 3;
const SLOT_RECORD_LEN: usize = 76;
const SLOT_NAME_MAX_LEN: usize = 32;
const DEVICE_NAME_MAX_LEN: usize = 24;
const STORAGE_HEADER_LEN: usize = 40;
const STORAGE_DATA_LEN: usize = STORAGE_HEADER_LEN + BOND_SLOT_COUNT * SLOT_RECORD_LEN + 4;
const STORAGE_COMMIT_LEN: usize = 4;

/// Each record is aligned to 4-byte boundary for Flash word writes.
pub const RECORD_SLOT_LEN: usize = (STORAGE_DATA_LEN + STORAGE_COMMIT_LEN).div_ceil(4) * 4;
pub const SLOTS_PER_PAGE: usize = (STORAGE_PAGE_SIZE as usize) / RECORD_SLOT_LEN;

#[repr(align(4))]
struct AlignedRecord([u8; RECORD_SLOT_LEN]);
pub struct PersistentState {
    pub active_profile: Option<usize>,
    pub bluetooth_enabled: bool,
    pub bonds: [Option<BondInformation>; BOND_SLOT_COUNT],
    pub cccd_flags: [u8; BOND_SLOT_COUNT],
    pub slot_names: [heapless::String<32>; BOND_SLOT_COUNT],
    pub device_name: heapless::String<24>,
}
type StorageRecord = (u32, PersistentState);

#[derive(Clone, Copy)]
pub struct StorageCursor {
    sequence: u32,
    page: u32,
    slot: Option<usize>,
    found: bool,
    durable_record: Option<[u8; RECORD_SLOT_LEN]>,
}

impl StorageCursor {
    pub const fn empty() -> Self {
        Self {
            sequence: 0,
            page: STORAGE_PAGE0,
            slot: None,
            found: false,
            durable_record: None,
        }
    }
}

#[derive(Debug)]
#[cfg_attr(target_arch = "arm", derive(defmt::Format))]
pub enum StorageError<E> {
    Io(E),
    Verify,
    Injected,
}

impl<E> From<E> for StorageError<E> {
    fn from(error: E) -> Self {
        Self::Io(error)
    }
}

impl StorageCursor {
    pub fn persistent_state(&self) -> Option<PersistentState> {
        self.durable_record
            .as_ref()
            .and_then(|record| decode_storage(record).map(|(_, state)| state))
    }
}

/// One scan chooses the durable state and append cursor from the same record.
pub async fn scan_storage<F: NorFlash>(
    flash: &mut F,
) -> Result<(StorageCursor, Option<PersistentState>), F::Error> {
    let mut cursor = StorageCursor::empty();
    let mut persistent = None;
    for &page_addr in &[STORAGE_PAGE0, STORAGE_PAGE1] {
        for slot in 0..SLOTS_PER_PAGE {
            let addr = page_addr + (slot * RECORD_SLOT_LEN) as u32;
            let mut buf = [0u8; RECORD_SLOT_LEN];
            flash.read(addr, &mut buf).await?;
            {
                if let Some((sequence, state)) = decode_storage(&buf) {
                    if !cursor.found || sequence.wrapping_sub(cursor.sequence) < 0x8000_0000 {
                        cursor = StorageCursor {
                            sequence,
                            page: page_addr,
                            slot: Some(slot),
                            found: true,
                            durable_record: Some(buf),
                        };
                        persistent = Some(state);
                    }
                }
            }
        }
    }
    Ok((cursor, persistent))
}

#[cfg(test)]
async fn scan_storage_cursor<F: NorFlash>(flash: &mut F) -> StorageCursor
where
    F::Error: core::fmt::Debug,
{
    scan_storage(flash).await.unwrap().0
}

#[cfg(test)]
async fn load_persistent_state<F: NorFlash>(flash: &mut F) -> Option<PersistentState>
where
    F::Error: core::fmt::Debug,
{
    scan_storage(flash).await.unwrap().1
}

/// Explicit development erasure; normal factory reset only appends a record.
pub async fn erase_storage<F: NorFlash>(
    flash: &mut F,
) -> Result<StorageCursor, StorageError<F::Error>> {
    for page in [STORAGE_PAGE0, STORAGE_PAGE1] {
        flash.erase(page, page + STORAGE_PAGE_SIZE).await?;
        let mut readback = [0u8; 64];
        for offset in (0..STORAGE_PAGE_SIZE).step_by(readback.len()) {
            flash.read(page + offset, &mut readback).await?;
            if readback.iter().any(|byte| *byte != 0xFF) {
                return Err(StorageError::Verify);
            }
        }
    }
    Ok(StorageCursor::empty())
}

/// Appends a new state record into the next available Flash slot (Ring-Buffer Wear-Leveling).
// The arguments mirror the independently borrowed runtime state fields and
// avoid copying bond material into a second 300-byte snapshot on the stack.
#[allow(clippy::too_many_arguments)]
pub async fn save_persistent_state_cached<F: NorFlash>(
    flash: &mut F,
    cursor: &mut StorageCursor,
    active_profile: Option<usize>,
    bluetooth_enabled: bool,
    bonds: &[Option<BondInformation>; BOND_SLOT_COUNT],
    cccd_flags: &[u8; BOND_SLOT_COUNT],
    slot_names: &[heapless::String<32>; BOND_SLOT_COUNT],
    device_name: &str,
) -> Result<(), StorageError<F::Error>> {
    #[cfg(target_arch = "arm")]
    if active_profile.is_none()
        && !bluetooth_enabled
        && bonds.iter().all(Option::is_none)
        && device_name == "Pager"
        && crate::faults::take(5)
    {
        *cursor = erase_storage(flash).await?;
    }
    let new_seq = if cursor.found {
        cursor.sequence.wrapping_add(1)
    } else {
        1
    };

    // 2. Determine target write page & slot
    let (mut target_page, mut target_slot) = match (cursor.found, cursor.slot) {
        (true, Some(slot)) if slot + 1 < SLOTS_PER_PAGE => (cursor.page, slot + 1),
        (true, _) => {
            // Active page is full: switch to opposite page
            let next_page = if cursor.page == STORAGE_PAGE0 {
                STORAGE_PAGE1
            } else {
                STORAGE_PAGE0
            };
            (next_page, 0)
        }
        _ => (STORAGE_PAGE0, 0),
    };

    // 3. Build and write the record
    let mut storage = AlignedRecord([0xFFu8; RECORD_SLOT_LEN]);
    let buf = &mut storage.0;

    buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
    buf[4] = STORAGE_VERSION;
    buf[5] = active_profile
        .filter(|slot| *slot < BOND_SLOT_COUNT)
        .map(|slot| slot as u8)
        .unwrap_or(0xFF);
    buf[6] = u8::from(bluetooth_enabled);
    buf[7] = 0;
    buf[8..12].copy_from_slice(&new_seq.to_le_bytes());

    let device_name = device_name.as_bytes();
    let device_name_len = device_name.len().min(DEVICE_NAME_MAX_LEN);
    buf[12] = device_name_len as u8;
    buf[13..13 + device_name_len].copy_from_slice(&device_name[..device_name_len]);
    for (stored, flags) in buf[37..40].iter_mut().zip(cccd_flags) {
        *stored = flags & 0b111;
    }

    for (i, bond) in bonds.iter().enumerate() {
        let start = STORAGE_HEADER_LEN + i * SLOT_RECORD_LEN;
        if let Some(bond) = bond {
            encode_bond(
                bond,
                &slot_names[i],
                &mut buf[start..start + SLOT_RECORD_LEN],
            );
        } else {
            buf[start] = 0;
        }
    }

    let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
    buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());

    // Compare encoded semantic fields, excluding sequence/CRC/commit. An
    // unchanged snapshot is already durable and does not wear flash.
    if cursor.durable_record.as_ref().is_some_and(|durable| {
        durable[..8] == buf[..8]
            && durable[12..STORAGE_DATA_LEN - 4] == buf[12..STORAGE_DATA_LEN - 4]
    }) {
        return Ok(());
    }
    // A failed body/commit can leave the next physical slot programmed.
    // Never retry a different record over those NOR bits; skip it or roll over.
    while target_slot != 0 && target_slot < SLOTS_PER_PAGE {
        let mut candidate = [0u8; RECORD_SLOT_LEN];
        flash
            .read(
                target_page + (target_slot * RECORD_SLOT_LEN) as u32,
                &mut candidate,
            )
            .await?;
        if candidate.iter().all(|byte| *byte == 0xFF) {
            break;
        }
        target_slot += 1;
    }
    if target_slot == SLOTS_PER_PAGE {
        target_page = if target_page == STORAGE_PAGE0 {
            STORAGE_PAGE1
        } else {
            STORAGE_PAGE0
        };
        target_slot = 0;
    }
    if target_slot == 0 {
        flash
            .erase(target_page, target_page + STORAGE_PAGE_SIZE)
            .await?;
    }

    let target_addr = target_page + (target_slot * RECORD_SLOT_LEN) as u32;
    #[cfg(target_arch = "arm")]
    if crate::faults::take(1) {
        flash.write(target_addr, &buf[..16]).await?;
        return Err(StorageError::Injected);
    }
    flash.write(target_addr, &buf[..STORAGE_DATA_LEN]).await?;
    #[cfg(target_arch = "arm")]
    if crate::faults::take(2) {
        return Err(StorageError::Injected);
    }
    flash
        .write(
            target_addr + STORAGE_DATA_LEN as u32,
            &STORAGE_COMMIT.to_le_bytes(),
        )
        .await?;
    storage.0[STORAGE_DATA_LEN..STORAGE_DATA_LEN + 4]
        .copy_from_slice(&STORAGE_COMMIT.to_le_bytes());
    let mut verified = [0u8; RECORD_SLOT_LEN];
    flash.read(target_addr, &mut verified).await?;
    if verified != storage.0 {
        return Err(StorageError::Verify);
    }
    *cursor = StorageCursor {
        sequence: new_seq,
        page: target_page,
        slot: Some(target_slot),
        found: true,
        durable_record: Some(storage.0),
    };
    Ok(())
}

fn decode_storage(buf: &[u8]) -> Option<StorageRecord> {
    if buf.len() < RECORD_SLOT_LEN
        || u32::from_le_bytes(buf[0..4].try_into().ok()?) != STORAGE_MAGIC
        || buf[4] != STORAGE_VERSION
        || u32::from_le_bytes(
            buf[STORAGE_DATA_LEN..STORAGE_DATA_LEN + STORAGE_COMMIT_LEN]
                .try_into()
                .ok()?,
        ) != STORAGE_COMMIT
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
    let active_profile = match buf[5] {
        slot @ 0..=2 => Some(slot as usize),
        0xFF => None,
        _ => return None,
    };
    let bluetooth_enabled = match buf[6] {
        0 => false,
        1 => true,
        _ => return None,
    };
    let mut bonds = [None, None, None];
    let cccd_flags: [u8; BOND_SLOT_COUNT] = buf[37..40].try_into().ok()?;
    if cccd_flags.iter().any(|flags| flags & !0b111 != 0) {
        return None;
    }
    let mut names = [
        heapless::String::new(),
        heapless::String::new(),
        heapless::String::new(),
    ];
    let mut device_name = heapless::String::<24>::new();
    let device_name_len = buf[12] as usize;
    if device_name_len > DEVICE_NAME_MAX_LEN {
        return None;
    }
    if let Ok(name) = core::str::from_utf8(&buf[13..13 + device_name_len]) {
        device_name.push_str(name).ok()?;
    } else {
        return None;
    }
    for (index, bond) in bonds.iter_mut().enumerate() {
        let start = STORAGE_HEADER_LEN + index * SLOT_RECORD_LEN;
        let (b, name) = decode_bond(&buf[start..start + SLOT_RECORD_LEN]);
        *bond = b;
        names[index] = name;
    }
    if device_name.is_empty() {
        let _ = device_name.push_str("Pager");
    }
    Some((
        u32::from_le_bytes(buf[8..12].try_into().ok()?),
        PersistentState {
            active_profile,
            bluetooth_enabled,
            bonds,
            cccd_flags,
            slot_names: names,
            device_name,
        },
    ))
}

fn encode_bond(bond: &BondInformation, name: &str, out: &mut [u8]) {
    out.fill(0xFF);
    out[0] = 1;
    out[1] = bond.identity.addr.kind.as_raw();
    out[2..8].copy_from_slice(bond.identity.addr.addr.raw());
    out[8] = 0; // Explicit absence; erased 0xFF is not a valid IRK tag.
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

    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len().min(SLOT_NAME_MAX_LEN);
    out[43] = name_len as u8;
    if name_len > 0 {
        out[44..44 + name_len].copy_from_slice(&name_bytes[..name_len]);
    }
}

fn decode_bond_raw(data: &[u8]) -> Option<BondInformation> {
    if data.len() < 43 || data[0] != 1 {
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

fn decode_bond(data: &[u8]) -> (Option<BondInformation>, heapless::String<32>) {
    let bond = decode_bond_raw(data);
    let mut name = heapless::String::new();
    if data.len() >= SLOT_RECORD_LEN && data[43] > 0 && data[43] <= SLOT_NAME_MAX_LEN as u8 {
        let len = data[43] as usize;
        if let Ok(s) = core::str::from_utf8(&data[44..44 + len]) {
            let _ = name.push_str(s);
        }
    }
    (bond, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_storage_async::nor_flash::{
        ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
    };

    #[derive(Clone, Copy, Debug)]
    struct MockError;

    impl NorFlashError for MockError {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    struct MockFlash {
        bytes: std::vec::Vec<u8>,
        writes_before_failure: Option<usize>,
        partial_bytes: usize,
        fail_read: bool,
        fail_erase: bool,
        writes_before_drop: Option<usize>,
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                bytes: std::vec![0xFF; 1024 * 1024],
                writes_before_failure: None,
                partial_bytes: 0,
                fail_read: false,
                fail_erase: false,
                writes_before_drop: None,
            }
        }
    }

    impl ErrorType for MockFlash {
        type Error = MockError;
    }

    impl ReadNorFlash for MockFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            if self.fail_read {
                return Err(MockError);
            }
            let start = offset as usize;
            bytes.copy_from_slice(&self.bytes[start..start + bytes.len()]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for MockFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            if self.fail_erase {
                self.bytes[from as usize..((from + to) / 2) as usize].fill(0xFF);
                return Err(MockError);
            }
            self.bytes[from as usize..to as usize].fill(0xFF);
            Ok(())
        }

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if let Some(remaining) = self.writes_before_failure.as_mut() {
                if *remaining == 0 {
                    let start = offset as usize;
                    let count = self.partial_bytes.min(bytes.len());
                    for (stored, new) in self.bytes[start..start + count].iter_mut().zip(bytes) {
                        *stored &= *new;
                    }
                    return Err(MockError);
                }
                *remaining -= 1;
            }
            if let Some(remaining) = self.writes_before_drop.as_mut() {
                if *remaining == 0 {
                    return Ok(());
                }
                *remaining -= 1;
            }
            let start = offset as usize;
            for (stored, new) in self.bytes[start..start + bytes.len()].iter_mut().zip(bytes) {
                *stored &= *new;
            }
            Ok(())
        }
    }

    #[test]
    fn logical_reset_appends_without_claiming_old_key_erasure() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            let (mut cursor, _) = scan_storage(&mut flash).await.unwrap();
            let mut raw = [0u8; 43];
            raw[0] = 1;
            raw[25..41].fill(0x42);
            raw[41] = 1;
            raw[42] = 1;
            let bonds = [Some(decode_bond_raw(&raw).unwrap()), None, None];
            let names = Default::default();
            save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                Some(0),
                true,
                &bonds,
                &[1, 0, 0],
                &names,
                "Bonded",
            )
            .await
            .unwrap();
            let old = flash.bytes[STORAGE_PAGE0 as usize..STORAGE_PAGE0 as usize + RECORD_SLOT_LEN]
                .to_vec();
            save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                None,
                false,
                &[None, None, None],
                &[0; 3],
                &names,
                "Pager",
            )
            .await
            .unwrap();
            let restored = load_persistent_state(&mut flash).await.unwrap();
            assert!(restored.bonds.iter().all(Option::is_none));
            assert_eq!(restored.device_name.as_str(), "Pager");
            assert_eq!(
                old,
                flash.bytes[STORAGE_PAGE0 as usize..STORAGE_PAGE0 as usize + RECORD_SLOT_LEN]
            );
            assert!(decode_storage(&old).unwrap().1.bonds[0].is_some());
        });
    }

    #[test]
    fn physical_erase_removes_all_old_records_and_preserves_other_partitions() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            flash.bytes[..STORAGE_PAGE0 as usize].fill(0x42);
            flash.bytes[STORAGE_PAGE0 as usize..].fill(0xA5);
            let cursor = erase_storage(&mut flash).await.unwrap();
            assert!(cursor.persistent_state().is_none());
            assert!(flash.bytes[..STORAGE_PAGE0 as usize]
                .iter()
                .all(|byte| *byte == 0x42));
            assert!(flash.bytes[STORAGE_PAGE0 as usize..]
                .iter()
                .all(|byte| *byte == 0xFF));
            assert!(scan_storage(&mut flash).await.unwrap().1.is_none());
        });
    }

    #[test]
    fn scan_read_error_is_not_empty_storage() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            flash.fail_read = true;
            assert!(scan_storage(&mut flash).await.is_err());
        });
    }

    #[test]
    fn every_partial_body_and_commit_recovers_then_accepts_new_record() {
        futures::executor::block_on(async {
            for failed_write in [0, 1] {
                let length = if failed_write == 0 {
                    STORAGE_DATA_LEN
                } else {
                    4
                };
                for partial_bytes in (0..length).step_by(4) {
                    let mut flash = MockFlash::new();
                    let (mut cursor, _) = scan_storage(&mut flash).await.unwrap();
                    let names = Default::default();
                    save_persistent_state_cached(
                        &mut flash,
                        &mut cursor,
                        Some(0),
                        true,
                        &[None, None, None],
                        &[0; 3],
                        &names,
                        "Durable",
                    )
                    .await
                    .unwrap();
                    flash.writes_before_failure = Some(failed_write);
                    flash.partial_bytes = partial_bytes;
                    assert!(save_persistent_state_cached(
                        &mut flash,
                        &mut cursor,
                        Some(1),
                        true,
                        &[None, None, None],
                        &[0; 3],
                        &names,
                        "Interrupted"
                    )
                    .await
                    .is_err());
                    let (mut recovered, state) = scan_storage(&mut flash).await.unwrap();
                    assert_eq!(state.unwrap().device_name.as_str(), "Durable");
                    flash.writes_before_failure = None;
                    save_persistent_state_cached(
                        &mut flash,
                        &mut recovered,
                        Some(2),
                        true,
                        &[None, None, None],
                        &[0; 3],
                        &names,
                        "Recovered",
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        load_persistent_state(&mut flash)
                            .await
                            .unwrap()
                            .device_name
                            .as_str(),
                        "Recovered"
                    );
                }
            }
        });
    }

    #[test]
    fn rollover_erase_failure_and_silent_commit_are_not_success() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            let (mut cursor, _) = scan_storage(&mut flash).await.unwrap();
            let names = Default::default();
            for index in 0..SLOTS_PER_PAGE {
                let mut name = heapless::String::<24>::new();
                core::fmt::write(&mut name, format_args!("Record{index}")).unwrap();
                save_persistent_state_cached(
                    &mut flash,
                    &mut cursor,
                    Some(0),
                    true,
                    &[None, None, None],
                    &[0; 3],
                    &names,
                    name.as_str(),
                )
                .await
                .unwrap();
            }
            let old_seq = cursor.sequence;
            flash.fail_erase = true;
            assert!(save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                Some(1),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "EraseFault"
            )
            .await
            .is_err());
            let (mut recovered, _) = scan_storage(&mut flash).await.unwrap();
            assert_eq!(recovered.sequence, old_seq);
            flash.fail_erase = false;
            flash.writes_before_drop = Some(1);
            assert!(matches!(
                save_persistent_state_cached(
                    &mut flash,
                    &mut recovered,
                    Some(1),
                    true,
                    &[None, None, None],
                    &[0; 3],
                    &names,
                    "SilentCommit"
                )
                .await,
                Err(StorageError::Verify)
            ));
            let (mut recovered, _) = scan_storage(&mut flash).await.unwrap();
            assert_eq!(recovered.sequence, old_seq);
            flash.writes_before_drop = None;
            save_persistent_state_cached(
                &mut flash,
                &mut recovered,
                Some(2),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "Recovered",
            )
            .await
            .unwrap();
            assert_eq!(
                load_persistent_state(&mut flash)
                    .await
                    .unwrap()
                    .device_name
                    .as_str(),
                "Recovered"
            );
        });
    }

    #[test]
    fn test_storage_crc_and_decode_roundtrip() {
        let mut buf = [0xFFu8; RECORD_SLOT_LEN];
        buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
        buf[4] = STORAGE_VERSION;
        buf[5] = 1; // active_slot
        buf[6] = 1; // bluetooth enabled
        buf[8..12].copy_from_slice(&105u32.to_le_bytes()); // seq
        buf[12] = 0; // default device name
        buf[37..40].fill(0);

        let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
        buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());
        buf[STORAGE_DATA_LEN..STORAGE_DATA_LEN + 4].copy_from_slice(&STORAGE_COMMIT.to_le_bytes());

        let decoded = decode_storage(&buf);
        assert!(decoded.is_some());
        let (seq, state) = decoded.unwrap();
        assert_eq!(seq, 105);
        assert_eq!(state.active_profile, Some(1));
        assert!(state.bluetooth_enabled);
        assert_eq!(state.bonds, [None, None, None]);
        assert_eq!(state.device_name.as_str(), "Pager");
        assert_eq!(
            state.slot_names,
            [
                heapless::String::<32>::new(),
                heapless::String::<32>::new(),
                heapless::String::<32>::new()
            ]
        );
    }

    #[test]
    fn scan_wrap_and_unchanged_snapshot_skip_flash() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            let mut cursor = StorageCursor::empty();
            cursor.sequence = u32::MAX - 1;
            cursor.found = true;
            let names = Default::default();
            for name in ["BeforeWrap", "Wrapped"] {
                save_persistent_state_cached(
                    &mut flash,
                    &mut cursor,
                    Some(0),
                    true,
                    &[None, None, None],
                    &[0; 3],
                    &names,
                    name,
                )
                .await
                .unwrap();
            }
            let (mut recovered, state) = scan_storage(&mut flash).await.unwrap();
            assert_eq!(recovered.sequence, 0);
            assert_eq!(state.unwrap().device_name.as_str(), "Wrapped");
            let bytes = flash.bytes.clone();
            flash.writes_before_failure = Some(0);
            save_persistent_state_cached(
                &mut flash,
                &mut recovered,
                Some(0),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "Wrapped",
            )
            .await
            .unwrap();
            assert_eq!(recovered.sequence, 0);
            assert_eq!(bytes, flash.bytes);
        });
    }

    #[test]
    fn subscriptions_survive_flash_commit_and_old_schema_is_refused() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            let mut cursor = scan_storage_cursor(&mut flash).await;
            let mut raw_bond = [0u8; 43];
            raw_bond[0] = 1;
            raw_bond[25..41].fill(0x42);
            raw_bond[41] = 1;
            raw_bond[42] = 1;
            let bond = decode_bond_raw(&raw_bond).unwrap();
            let bonds = [Some(bond.clone()), Some(bond.clone()), Some(bond)];
            save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                Some(0),
                true,
                &bonds,
                &[1, 2, 4],
                &Default::default(),
                "Subscriptions",
            )
            .await
            .unwrap();
            let restored = load_persistent_state(&mut flash).await.unwrap();
            assert_eq!(restored.cccd_flags, [1, 2, 4]);
            assert_eq!(restored.bonds, bonds);
            // A correctly committed and CRC-valid old record still must not
            // be decoded using the changed development schema.
            let start = STORAGE_PAGE0 as usize;
            let buf = &mut flash.bytes[start..start + RECORD_SLOT_LEN];
            buf[..4].copy_from_slice(&0x3653_4750u32.to_le_bytes());
            buf[4] = 6;
            let crc = crc32_finalize(crc32_update(CRC32_INIT, &buf[..STORAGE_DATA_LEN - 4]));
            buf[STORAGE_DATA_LEN - 4..STORAGE_DATA_LEN].copy_from_slice(&crc.to_le_bytes());
            assert!(load_persistent_state(&mut flash).await.is_none());
        });
    }

    #[test]
    fn interrupted_commit_preserves_previous_record() {
        futures::executor::block_on(async {
            let mut flash = MockFlash::new();
            let mut cursor = scan_storage_cursor(&mut flash).await;
            let names = Default::default();
            save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                Some(0),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "First",
            )
            .await
            .unwrap();

            flash.writes_before_failure = Some(1);
            assert!(save_persistent_state_cached(
                &mut flash,
                &mut cursor,
                Some(1),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "Interrupted",
            )
            .await
            .is_err());

            let restored = load_persistent_state(&mut flash).await.unwrap();
            assert_eq!(restored.active_profile, Some(0));
            assert_eq!(restored.device_name.as_str(), "First");
            let (mut recovered, _) = scan_storage(&mut flash).await.unwrap();
            flash.writes_before_failure = None;
            save_persistent_state_cached(
                &mut flash,
                &mut recovered,
                Some(2),
                true,
                &[None, None, None],
                &[0; 3],
                &names,
                "Recovery",
            )
            .await
            .unwrap();
            assert_eq!(recovered.slot, Some(2));
            let recovered_state = load_persistent_state(&mut flash).await.unwrap();
            assert_eq!(recovered_state.device_name.as_str(), "Recovery");
            assert_eq!(recovered_state.active_profile, Some(2));
        });
    }
}
