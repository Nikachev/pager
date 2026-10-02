#![no_std]

/// Reject any ACL read/write denial intersecting the flash being replaced.
/// ACL entries cover nonnegative addresses, so a nonempty region overlaps
/// [0, end) exactly when its start is below end.
pub fn acl_blocks_migration(address: u32, size: u32, permissions: u32, end: u32) -> bool {
    const DENIED: u32 = (1 << 1) | (1 << 2);
    size != 0 && permissions & DENIED != 0 && address < end
}

#[cfg(test)]
mod tests {
    use super::acl_blocks_migration;

    #[test]
    fn rejects_mbr_and_softdevice_protection() {
        assert!(acl_blocks_migration(0, 4096, 2, 0x27000));
        assert!(acl_blocks_migration(0x1000, 4096, 4, 0x27000));
        assert!(acl_blocks_migration(0x26000, 4096, 6, 0x27000));
    }

    #[test]
    fn allows_unprotected_flash_and_protected_factory_bootloader() {
        assert!(!acl_blocks_migration(0, 4096, 0, 0x27000));
        assert!(!acl_blocks_migration(0, 0, 6, 0x27000));
        assert!(!acl_blocks_migration(0x27000, 4096, 2, 0x27000));
        assert!(!acl_blocks_migration(0xF4000, 0xC000, 6, 0x27000));
    }
}
