/// IEEE CRC-32 used by both OTA transports to detect corrupted staged data.
pub const CRC32_INIT: u32 = 0xFFFF_FFFF;
pub const PACKAGE_MAGIC: [u8; 8] = *b"PGRFW001";
pub const MANIFEST_LEN: usize = 120;
pub const MANIFEST_PAGE_SIZE: usize = 4096;

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

/// Parse exactly eight lowercase/uppercase hexadecimal digits. This is used
/// for the OTA checksum header and deliberately rejects prefixes and suffixes.
pub fn parse_hex_u32(value: &[u8]) -> Option<u32> {
    if value.len() != 8 {
        return None;
    }
    value.iter().try_fold(0u32, |result, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        Some((result << 4) | u32::from(digit))
    })
}

/// Validate transport-visible package structure before rebooting into the
/// bootloader. Signature validation remains the bootloader's responsibility.
pub fn valid_package_envelope(
    manifest_page: &[u8],
    package_len: usize,
    expected_slot: u32,
    max_package_len: usize,
) -> bool {
    if manifest_page.len() != MANIFEST_PAGE_SIZE
        || package_len <= MANIFEST_PAGE_SIZE
        || package_len > max_package_len
    {
        return false;
    }
    let state = u32::from_le_bytes(match manifest_page[0..4].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    });
    let image_len = u32::from_le_bytes(match manifest_page[16..20].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    }) as usize;
    let target_slot = u32::from_le_bytes(match manifest_page[20..24].try_into() {
        Ok(value) => value,
        Err(_) => return false,
    });
    state == u32::MAX
        && manifest_page[4..12] == PACKAGE_MAGIC
        && target_slot == expected_slot
        && image_len > 0
        && image_len <= max_package_len - MANIFEST_PAGE_SIZE
        && package_len == MANIFEST_PAGE_SIZE + image_len
        && manifest_page[MANIFEST_LEN..]
            .iter()
            .all(|byte| *byte == 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_vector_incrementally() {
        let crc = crc32_update(CRC32_INIT, b"1234");
        let crc = crc32_update(crc, b"56789");
        assert_eq!(crc32_finalize(crc), 0xCBF4_3926);
    }

    #[test]
    fn checksum_parser_is_exact() {
        assert_eq!(parse_hex_u32(b"deadBEEF"), Some(0xDEAD_BEEF));
        assert_eq!(parse_hex_u32(b"deadbeef!"), None);
        assert_eq!(parse_hex_u32(b"0xdeadbeef"), None);
    }

    #[test]
    fn manifest_validation_rejects_wrong_slot_and_dirty_padding() {
        let mut page = [0xFF; MANIFEST_PAGE_SIZE];
        page[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        page[4..12].copy_from_slice(&PACKAGE_MAGIC);
        page[16..20].copy_from_slice(&4u32.to_le_bytes());
        page[20..24].copy_from_slice(&1u32.to_le_bytes());
        assert!(valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            1,
            500_000
        ));
        assert!(!valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            0,
            500_000
        ));
        page[MANIFEST_LEN] = 0;
        assert!(!valid_package_envelope(
            &page,
            MANIFEST_PAGE_SIZE + 4,
            1,
            500_000
        ));
    }
}
