//! SCSI Command Handler for USB Mass Storage (SCSI Bulk-Only Transport)

use usb_device::bus::UsbBus;
use usbd_storage::subclass::scsi::{Scsi, ScsiCommand};
use usbd_storage::transport::bbb::BulkOnly;

pub const INQUIRY_VENDOR: &[u8; 8] = b"Nikachev";
pub const INQUIRY_PRODUCT: &[u8; 16] = b"Pager Boot Drive";
pub const INQUIRY_REVISION: &[u8; 4] = b"1.00";

pub fn handle_scsi_inquiry<Bus: UsbBus, Buf: core::borrow::BorrowMut<[u8]>>(
    mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'_, Bus, Buf>>>,
    alloc_len: u16,
) {
    let mut resp = [0u8; 36];
    resp[0] = 0x00; // Direct Access Block Device
    resp[1] = 0x80; // Removable Media (RMB = 1) for macOS/Windows compatibility
    resp[2] = 0x02; // SPC-2
    resp[3] = 0x02; // Response data format
    resp[4] = 31;
    resp[8..16].copy_from_slice(INQUIRY_VENDOR);
    resp[16..32].copy_from_slice(INQUIRY_PRODUCT);
    resp[32..36].copy_from_slice(INQUIRY_REVISION);
    let send_len = (alloc_len as usize).min(36);
    let _ = cmd.write_data(&resp[..send_len]);
    cmd.pass();
}

pub fn handle_scsi_read_capacity<Bus: UsbBus, Buf: core::borrow::BorrowMut<[u8]>>(
    mut cmd: usbd_storage::subclass::Command<'_, ScsiCommand, Scsi<BulkOnly<'_, Bus, Buf>>>,
) {
    let mut resp = [0u8; 8];
    resp[0..4].copy_from_slice(&131071u32.to_be_bytes()); // Last LBA = 131071 (64 MB)
    resp[4..8].copy_from_slice(&512u32.to_be_bytes());
    let _ = cmd.write_data(&resp);
    cmd.pass();
}
