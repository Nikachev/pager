//! Virtual FAT16 File System Generator for Pager UF2 Bootloader

use crate::memory_map::FIRMWARE_START;

pub const VOLUME_LABEL: &[u8; 11] = b"PAGER_BOOT ";
pub const TOTAL_SECTORS: u32 = 131072; // 64 MB virtual disk

pub fn get_virtual_fat_sector(lba: u32, buf: &mut [u8]) {
    buf.fill(0);
    match lba {
        0 => {
            // LBA 0: Standard UF2 Boot Sector (BPB) for 64 MB FAT16 (2 sectors/cluster = 65277 clusters)
            buf[0] = 0xEB;
            buf[1] = 0x3C;
            buf[2] = 0x90;
            buf[3..11].copy_from_slice(b"MSDOS5.0");
            buf[11..13].copy_from_slice(&512u16.to_le_bytes()); // 512 bytes/sector
            buf[13] = 2; // 2 sectors/cluster (Required for FAT16 <= 65524 clusters on 64MB volume)
            buf[14..16].copy_from_slice(&1u16.to_le_bytes()); // 1 reserved sector (LBA 0)
            buf[16] = 2; // 2 FAT tables
            buf[17..19].copy_from_slice(&64u16.to_le_bytes()); // 64 root dir entries (4 sectors)
            buf[19..21].copy_from_slice(&0u16.to_le_bytes()); // 0 (use 32-bit total sectors)
            buf[21] = 0xF8; // Media descriptor: Fixed Disk / Removable
            buf[22..24].copy_from_slice(&256u16.to_le_bytes()); // 256 sectors per FAT
            buf[24..26].copy_from_slice(&32u16.to_le_bytes()); // 32 sectors/track
            buf[26..28].copy_from_slice(&64u16.to_le_bytes()); // 64 heads
            buf[28..32].copy_from_slice(&0u32.to_le_bytes()); // 0 hidden sectors
            buf[32..36].copy_from_slice(&TOTAL_SECTORS.to_le_bytes()); // 131072 total sectors (64 MB)
            buf[36] = 0x80; // Drive number
            buf[38] = 0x29; // Extended Boot Signature
            buf[39..43].copy_from_slice(&0x12345678u32.to_le_bytes()); // Serial
            buf[43..54].copy_from_slice(VOLUME_LABEL); // Volume Label (11 bytes)
            buf[54..62].copy_from_slice(b"FAT16   "); // FS Type (8 bytes)
            buf[510] = 0x55;
            buf[511] = 0xAA;
        }
        1..=512 => {
            // LBA 1..256 (FAT 1) & LBA 257..512 (FAT 2): 256-sector FAT Table
            let fat_sec_idx = if lba <= 256 { lba - 1 } else { lba - 257 };
            let start_entry = (fat_sec_idx * 256) as usize;
            for i in 0..256 {
                let entry_idx = start_entry + i;
                let val: u16 = get_fat16_entry(entry_idx as u32);
                buf[i * 2..i * 2 + 2].copy_from_slice(&val.to_le_bytes());
            }
        }
        513 => {
            // LBA 513: Root Directory Sector 1 (64 entries total)
            // Entry 0: Volume Label
            buf[0..11].copy_from_slice(VOLUME_LABEL); // 11 bytes
            buf[11] = 0x08; // Volume Label attribute

            // Entry 1: INFO_UF2.TXT
            let e1 = &mut buf[32..64];
            e1[0..11].copy_from_slice(b"INFO_UF2TXT");
            e1[11] = 0x20; // Normal file (Archive)
            e1[26..28].copy_from_slice(&2u16.to_le_bytes()); // Cluster 2 (LBA 517)
            e1[28..32].copy_from_slice(&120u32.to_le_bytes()); // 120 bytes

            // Entry 2: INDEX.HTM
            let e2 = &mut buf[64..96];
            e2[0..11].copy_from_slice(b"INDEX   HTM");
            e2[11] = 0x20; // Normal file (Archive)
            e2[26..28].copy_from_slice(&3u16.to_le_bytes()); // Cluster 3 (LBA 519)
            e2[28..32].copy_from_slice(&105u32.to_le_bytes()); // 105 bytes

            // Entry 3: CURRENT.UF2
            let e3 = &mut buf[96..128];
            e3[0..11].copy_from_slice(b"CURRENT UF2");
            e3[11] = 0x20; // Normal file (Archive)
            e3[26..28].copy_from_slice(&4u16.to_le_bytes()); // Cluster 4 (LBA 521)
            e3[28..32].copy_from_slice(&524288u32.to_le_bytes()); // 512 KiB
        }
        517 => {
            // LBA 517: Cluster 2 -> INFO_UF2.TXT
            let (dev0, dev1) = unsafe {
                (
                    core::ptr::read_volatile(0x1000_0060 as *const u32),
                    core::ptr::read_volatile(0x1000_0064 as *const u32),
                )
            };
            let mut info_buf = [0u8; 256];
            let mut off = 0;
            let header = b"UF2 Bootloader v0.1.0\r\nModel: Pager nRF52840\r\nBoard-ID: NRF52840-PAG-v1\r\nSerial: ";
            info_buf[..header.len()].copy_from_slice(header);
            off += header.len();

            let write_hex = |val: u32, buf: &mut [u8], off: &mut usize| {
                const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";
                for i in (0..8).rev() {
                    let nibble = ((val >> (i * 4)) & 0xF) as usize;
                    buf[*off] = HEX_CHARS[nibble];
                    *off += 1;
                }
            };
            write_hex(dev0, &mut info_buf, &mut off);
            write_hex(dev1, &mut info_buf, &mut off);

            let footer = b"\r\nDate: Aug  8 2026\r\n";
            info_buf[off..off + footer.len()].copy_from_slice(footer);
            off += footer.len();

            let len = off.min(512);
            buf[..len].copy_from_slice(&info_buf[..len]);
        }
        519 => {
            // LBA 519: Cluster 3 -> INDEX.HTM
            let html = b"STATUS_OK\n";
            let len = html.len().min(512);
            buf[..len].copy_from_slice(&html[..len]);
        }
        521..=1544 => {
            // LBA 521..1544: Cluster 4..515 -> CURRENT.UF2 (512 KiB, 1024 sectors)
            let block_no = lba - 521;
            let num_blocks = 1024u32;
            let target_addr = FIRMWARE_START + (block_no * 256);

            let magic0 = 0x0A324655u32;
            let magic1 = 0x9E5D5157u32;
            let flags = 0x00002000u32; // Family ID present
            let payload_size = 256u32;
            let family_id = 0xADA52840u32; // nRF52840 Family ID
            let magic2 = 0x0AB16F30u32;

            buf[0..4].copy_from_slice(&magic0.to_le_bytes());
            buf[4..8].copy_from_slice(&magic1.to_le_bytes());
            buf[8..12].copy_from_slice(&flags.to_le_bytes());
            buf[12..16].copy_from_slice(&target_addr.to_le_bytes());
            buf[16..20].copy_from_slice(&payload_size.to_le_bytes());
            buf[20..24].copy_from_slice(&block_no.to_le_bytes());
            buf[24..28].copy_from_slice(&num_blocks.to_le_bytes());
            buf[28..32].copy_from_slice(&family_id.to_le_bytes());

            // Copy Flash payload at target_addr
            let flash_slice = unsafe { core::slice::from_raw_parts(target_addr as *const u8, 256) };
            buf[32..288].copy_from_slice(flash_slice);
            buf[508..512].copy_from_slice(&magic2.to_le_bytes());
        }
        _ => {
            // All other FAT, Root Dir, and Data sectors return 0s
        }
    }
}

pub fn get_fat16_entry(cluster: u32) -> u16 {
    if cluster == 0 {
        0xFFF8
    } else if (1..=3).contains(&cluster) {
        0xFFFF // Reserved, INFO_UF2.TXT, INDEX.HTM
    } else if (4..515).contains(&cluster) {
        (cluster + 1) as u16 // CURRENT.UF2 cluster chain (512 clusters = 512 KiB)
    } else if cluster == 515 {
        0xFFFF // CURRENT.UF2 EOF
    } else {
        0x0000 // Free cluster
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bpb_structure() {
        let mut sector = [0u8; 512];
        get_virtual_fat_sector(0, &mut sector);
        assert_eq!(&sector[43..54], b"PAGER_BOOT ");
        assert_eq!(sector[510], 0x55);
        assert_eq!(sector[511], 0xAA);
        assert_eq!(u16::from_le_bytes([sector[11], sector[12]]), 512); // bytes/sector
        assert_eq!(sector[13], 2); // sectors/cluster
    }

    #[test]
    fn test_root_dir_entries() {
        let mut sector = [0u8; 512];
        get_virtual_fat_sector(513, &mut sector);
        assert_eq!(&sector[0..11], b"PAGER_BOOT ");
        assert_eq!(&sector[32..43], b"INFO_UF2TXT");
        assert_eq!(&sector[64..75], b"INDEX   HTM");
        assert_eq!(&sector[96..107], b"CURRENT UF2");
    }

    #[test]
    fn test_fat16_cluster_chain() {
        assert_eq!(get_fat16_entry(0), 0xFFF8);
        assert_eq!(get_fat16_entry(2), 0xFFFF);
        assert_eq!(get_fat16_entry(4), 5);
        assert_eq!(get_fat16_entry(515), 0xFFFF);
        assert_eq!(get_fat16_entry(1000), 0x0000);
    }
}
