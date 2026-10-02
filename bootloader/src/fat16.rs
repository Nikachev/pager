//! Minimal read-only FAT16 view exposed by the UF2 bootloader.

pub const VOLUME_LABEL: &[u8; 11] = b"PAGER_BOOT ";
pub const TOTAL_SECTORS: u32 = 131_072;
const INFO_HEADER: &[u8] =
    b"UF2 Bootloader v0.3.0\r\nModel: Pager nRF52840\r\nBoard-ID: NRF52840-PAGER\r\nSerial: ";
const INFO_KIND_PREFIX: &[u8] = b"\r\nKind: ";
const INFO_DATE_PREFIX: &[u8] = b"\r\nDate: ";
#[cfg(feature = "board-nice-nano-v2")]
const BOARD: &str = "nice-nano-v2";
#[cfg(feature = "board-xiao-nrf52840")]
const BOARD: &str = "xiao-nrf52840";
const BOARD_PREFIX: &[u8] = b"\r\nBoard: ";
const VERSION_PREFIX: &[u8] = b"\r\nVersion: ";
const HASH_PREFIX: &[u8] = b"\r\nPartition-SHA256: ";
const CAPABILITIES: &[u8] = b"\r\nCapabilities: signed-uf2,bootloader-updater-v1\r\n";

fn info_file_len() -> u32 {
    (INFO_HEADER.len()
        + 16
        + INFO_KIND_PREFIX.len()
        + crate::public_key::BOOTLOADER_KIND.len()
        + INFO_DATE_PREFIX.len()
        + option_env!("BUILD_DATE").unwrap_or("unknown").len()
        + BOARD_PREFIX.len()
        + BOARD.len()
        + VERSION_PREFIX.len()
        + env!("PAGER_BOOTLOADER_VERSION").len()
        + HASH_PREFIX.len()
        + 64
        + CAPABILITIES.len()) as u32
}

pub fn get_virtual_fat_sector(lba: u32, buf: &mut [u8]) {
    buf.fill(0);
    match lba {
        0 => {
            buf[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
            buf[3..11].copy_from_slice(b"MSDOS5.0");
            buf[11..13].copy_from_slice(&512u16.to_le_bytes());
            buf[13] = 2;
            buf[14..16].copy_from_slice(&1u16.to_le_bytes());
            buf[16] = 2;
            buf[17..19].copy_from_slice(&64u16.to_le_bytes());
            buf[21] = 0xF8;
            buf[22..24].copy_from_slice(&256u16.to_le_bytes());
            buf[24..26].copy_from_slice(&32u16.to_le_bytes());
            buf[26..28].copy_from_slice(&64u16.to_le_bytes());
            buf[32..36].copy_from_slice(&TOTAL_SECTORS.to_le_bytes());
            buf[36] = 0x80;
            buf[38] = 0x29;
            buf[39..43].copy_from_slice(&0x1234_5678u32.to_le_bytes());
            buf[43..54].copy_from_slice(VOLUME_LABEL);
            buf[54..62].copy_from_slice(b"FAT16   ");
            buf[510..512].copy_from_slice(&[0x55, 0xAA]);
        }
        1..=512 => {
            let fat_sector = if lba <= 256 { lba - 1 } else { lba - 257 };
            let first = fat_sector * 256;
            for offset in 0..256 {
                let value = get_fat16_entry(first + offset);
                let start = offset as usize * 2;
                buf[start..start + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
        513 => {
            buf[0..11].copy_from_slice(VOLUME_LABEL);
            buf[11] = 0x08;
            let entry = &mut buf[32..64];
            entry[0..11].copy_from_slice(b"INFO_UF2TXT");
            entry[11] = 0x20;
            entry[26..28].copy_from_slice(&2u16.to_le_bytes());
            entry[28..32].copy_from_slice(&info_file_len().to_le_bytes());
        }
        517 => write_info(buf),
        _ => {}
    }
}

fn write_info(buf: &mut [u8]) {
    let (device0, device1) = unsafe {
        (
            core::ptr::read_volatile(0x1000_0060 as *const u32),
            core::ptr::read_volatile(0x1000_0064 as *const u32),
        )
    };
    let mut offset = 0;
    append(buf, &mut offset, INFO_HEADER);
    append_hex(buf, &mut offset, device1);
    append_hex(buf, &mut offset, device0);
    append(buf, &mut offset, INFO_KIND_PREFIX);
    append(
        buf,
        &mut offset,
        crate::public_key::BOOTLOADER_KIND.as_bytes(),
    );
    append(buf, &mut offset, INFO_DATE_PREFIX);
    append(
        buf,
        &mut offset,
        option_env!("BUILD_DATE").unwrap_or("unknown").as_bytes(),
    );
    append(buf, &mut offset, BOARD_PREFIX);
    append(buf, &mut offset, BOARD.as_bytes());
    append(buf, &mut offset, VERSION_PREFIX);
    append(
        buf,
        &mut offset,
        env!("PAGER_BOOTLOADER_VERSION").as_bytes(),
    );
    append(buf, &mut offset, HASH_PREFIX);
    use sha2::{Digest, Sha256};
    // Hash the entire partition, including the erased tail, avoiding a
    // self-referential embedded image hash or linker-dependent image length.
    let mut hasher = Sha256::new();
    for address in (0..0xC000u32).step_by(4) {
        crate::feed_inherited_watchdog();
        let word = unsafe { core::ptr::read_volatile(address as *const u32) };
        hasher.update(word.to_le_bytes());
    }
    let digest = hasher.finalize();
    for chunk in digest.chunks_exact(4) {
        append_hex(
            buf,
            &mut offset,
            u32::from_be_bytes(chunk.try_into().unwrap()),
        );
    }
    append(buf, &mut offset, CAPABILITIES);
}

fn append(buf: &mut [u8], offset: &mut usize, value: &[u8]) {
    buf[*offset..*offset + value.len()].copy_from_slice(value);
    *offset += value.len();
}

fn append_hex(buf: &mut [u8], offset: &mut usize, value: u32) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for shift in (0..8).rev() {
        buf[*offset] = HEX[((value >> (shift * 4)) & 0xF) as usize];
        *offset += 1;
    }
}

pub fn get_fat16_entry(cluster: u32) -> u16 {
    match cluster {
        0 => 0xFFF8,
        1 | 2 => 0xFFFF,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpb_and_root_directory_are_consistent() {
        let mut sector = [0u8; 512];
        get_virtual_fat_sector(0, &mut sector);
        assert_eq!(&sector[43..54], VOLUME_LABEL);
        assert_eq!(&sector[510..512], &[0x55, 0xAA]);

        get_virtual_fat_sector(513, &mut sector);
        assert_eq!(&sector[32..43], b"INFO_UF2TXT");
        assert_eq!(
            u32::from_le_bytes(sector[60..64].try_into().unwrap()),
            info_file_len()
        );
        assert!(sector[64..].iter().all(|byte| *byte == 0));
        assert_eq!(get_fat16_entry(2), 0xFFFF);
        assert_eq!(get_fat16_entry(3), 0);
    }
}
