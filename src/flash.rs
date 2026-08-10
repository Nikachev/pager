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

const STORAGE_MAGIC: u32 = 0x3653_4750; // "PGS6"
const STORAGE_VERSION: u8 = 6;
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
}

impl StorageCursor {
    const fn empty() -> Self {
        Self {
            sequence: 0,
            page: STORAGE_PAGE0,
            slot: None,
            found: false,
        }
    }
}

pub async fn scan_storage_cursor<F: NorFlash>(flash: &mut F) -> StorageCursor {
    let mut cursor = StorageCursor::empty();
    for &page_addr in &[STORAGE_PAGE0, STORAGE_PAGE1] {
        for slot in 0..SLOTS_PER_PAGE {
            let addr = page_addr + (slot * RECORD_SLOT_LEN) as u32;
            let mut buf = [0u8; RECORD_SLOT_LEN];
            if flash.read(addr, &mut buf).await.is_ok() {
                if let Some((sequence, _)) = decode_storage(&buf) {
                    if !cursor.found || sequence.wrapping_sub(cursor.sequence) < 0x8000_0000 {
                        cursor = StorageCursor {
                            sequence,
                            page: page_addr,
                            slot: Some(slot),
                            found: true,
                        };
                    }
                }
            }
        }
    }
    cursor
}

/// Scans both STORAGE pages for the record with the highest sequence number.
pub async fn load_persistent_state<F: NorFlash>(flash: &mut F) -> Option<PersistentState> {
    let mut best_record: Option<StorageRecord> = None;

    for &page_addr in &[STORAGE_PAGE0, STORAGE_PAGE1] {
        for slot in 0..SLOTS_PER_PAGE {
            let addr = page_addr + (slot * RECORD_SLOT_LEN) as u32;
            let mut header = [0u8; STORAGE_HEADER_LEN];
            if flash.read(addr, &mut header).await.is_err() {
                continue;
            }

            if header.iter().all(|&b| b == 0xFF) {
                continue;
            }

            let mut buf = [0u8; RECORD_SLOT_LEN];
            buf[..STORAGE_HEADER_LEN].copy_from_slice(&header);
            if flash
                .read(
                    addr + STORAGE_HEADER_LEN as u32,
                    &mut buf[STORAGE_HEADER_LEN..],
                )
                .await
                .is_ok()
            {
                if let Some(record) = decode_storage(&buf) {
                    best_record = select_storage(best_record, Some(record));
                }
            }
        }
    }

    best_record.map(|(_, state)| state)
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
    _cccd_flags: &[u8; BOND_SLOT_COUNT],
    slot_names: &[heapless::String<32>; BOND_SLOT_COUNT],
    device_name: &str,
) -> Result<(), F::Error> {
    let new_seq = if cursor.found {
        cursor.sequence.wrapping_add(1)
    } else {
        1
    };

    // 2. Determine target write page & slot
    let (target_page, target_slot) = match (cursor.found, cursor.slot) {
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

    // If starting a new page (slot 0), erase target page first
    if target_slot == 0 {
        flash
            .erase(target_page, target_page + STORAGE_PAGE_SIZE)
            .await?;
    }

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
    buf[37..40].fill(0);

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

    let target_addr = target_page + (target_slot * RECORD_SLOT_LEN) as u32;
    flash.write(target_addr, &buf[..STORAGE_DATA_LEN]).await?;
    flash
        .write(
            target_addr + STORAGE_DATA_LEN as u32,
            &STORAGE_COMMIT.to_le_bytes(),
        )
        .await?;
    *cursor = StorageCursor {
        sequence: new_seq,
        page: target_page,
        slot: Some(target_slot),
        found: true,
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
    let cccd_flags = [0; BOND_SLOT_COUNT];
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

fn encode_bond(bond: &BondInformation, name: &str, out: &mut [u8]) {
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
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                bytes: std::vec![0xFF; 1024 * 1024],
                writes_before_failure: None,
            }
        }
    }

    impl ErrorType for MockFlash {
        type Error = MockError;
    }

    impl ReadNorFlash for MockFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
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
            self.bytes[from as usize..to as usize].fill(0xFF);
            Ok(())
        }

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if let Some(remaining) = self.writes_before_failure.as_mut() {
                if *remaining == 0 {
                    return Err(MockError);
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
    fn test_storage_crc_and_decode_roundtrip() {
        let mut buf = [0xFFu8; RECORD_SLOT_LEN];
        buf[0..4].copy_from_slice(&STORAGE_MAGIC.to_le_bytes());
        buf[4] = STORAGE_VERSION;
        buf[5] = 1; // active_slot
        buf[6] = 1; // bluetooth enabled
        buf[8..12].copy_from_slice(&105u32.to_le_bytes()); // seq
        buf[12] = 0; // default device name

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
    fn test_select_storage_sequence_wrap() {
        let empty_names = [
            heapless::String::new(),
            heapless::String::new(),
            heapless::String::new(),
        ];
        let rec1 = Some((
            100u32,
            PersistentState {
                active_profile: Some(0),
                bluetooth_enabled: true,
                bonds: [None, None, None],
                cccd_flags: [0; 3],
                slot_names: empty_names.clone(),
                device_name: heapless::String::<24>::try_from("Pager").unwrap(),
            },
        ));
        let rec2 = Some((
            101u32,
            PersistentState {
                active_profile: Some(1),
                bluetooth_enabled: true,
                bonds: [None, None, None],
                cccd_flags: [0; 3],
                slot_names: empty_names.clone(),
                device_name: heapless::String::<24>::try_from("Pager").unwrap(),
            },
        ));
        assert_eq!(select_storage(rec1, rec2).unwrap().0, 101);

        let rec_wrapped = Some((
            1u32,
            PersistentState {
                active_profile: Some(0),
                bluetooth_enabled: true,
                bonds: [None, None, None],
                cccd_flags: [0; 3],
                slot_names: empty_names.clone(),
                device_name: heapless::String::<24>::try_from("Pager").unwrap(),
            },
        ));
        let rec_old = Some((
            0xFFFF_FFFFu32,
            PersistentState {
                active_profile: Some(1),
                bluetooth_enabled: true,
                bonds: [None, None, None],
                cccd_flags: [0; 3],
                slot_names: empty_names.clone(),
                device_name: heapless::String::<24>::try_from("Pager").unwrap(),
            },
        ));
        assert_eq!(select_storage(rec_old, rec_wrapped).unwrap().0, 1);
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
        });
    }
}
