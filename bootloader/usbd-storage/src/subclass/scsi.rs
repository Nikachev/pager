//! USB SCSI

use crate::CLASS_MASS_STORAGE;
use crate::fmt::trace;
use crate::transport::Transport;
use core::fmt::Debug;
use num_enum::TryFromPrimitive;
use usb_device::UsbError;
use usb_device::bus::InterfaceNumber;
use usb_device::bus::UsbBus;
use usb_device::class::{ControlIn, UsbClass};
use usb_device::descriptor::DescriptorWriter;

#[cfg(feature = "bbb")]
use {
    crate::subclass::Command,
    crate::transport::TransportError,
    crate::transport::bbb::{BulkOnly, BulkOnlyError},
    core::borrow::BorrowMut,
    usb_device::bus::UsbBusAllocator,
};

/// SCSI device subclass code
pub const SUBCLASS_SCSI: u8 = 0x06; // SCSI Transparent command set

/* SCSI codes */

/* SPC */
const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const MODE_SENSE_6: u8 = 0x1A;
const MODE_SENSE_10: u8 = 0x5A;
const START_STOP_UNIT: u8 = 0x1B;

/* SBC */
const READ_10: u8 = 0x28;
const READ_16: u8 = 0x88;
const READ_CAPACITY_10: u8 = 0x25;
const READ_CAPACITY_16: u8 = 0x9E;
const WRITE_10: u8 = 0x2A;
const SYNCHRONIZE_CACHE_10: u8 = 0x35;

/* MMC */
const READ_FORMAT_CAPACITIES: u8 = 0x23;

/// SCSI command
///
/// Refer to specifications (SPC,SAM,SBC,MMC,etc.)
#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ScsiCommand {
    Unknown,

    /* SPC */
    Inquiry {
        evpd: bool,
        page_code: u8,
        alloc_len: u16,
    },
    TestUnitReady,
    StartStop {
        start: bool,
        load_eject: bool,
    },
    SynchronizeCache,
    RequestSense {
        desc: bool,
        alloc_len: u8,
    },
    ModeSense6 {
        dbd: bool,
        page_control: PageControl,
        page_code: u8,
        subpage_code: u8,
        alloc_len: u8,
    },
    ModeSense10 {
        dbd: bool,
        page_control: PageControl,
        page_code: u8,
        subpage_code: u8,
        alloc_len: u16,
    },

    /* SBC */
    ReadCapacity10,
    ReadCapacity16 {
        alloc_len: u32,
    },
    Read {
        lba: u64,
        len: u64,
    },
    Write {
        lba: u64,
        len: u64,
    },

    /* MMC */
    ReadFormatCapacities {
        alloc_len: u16,
    },
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, TryFromPrimitive)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PageControl {
    CurrentValues = 0b00,
    ChangeableValues = 0b01,
    DefaultValues = 0b10,
    SavedValues = 0b11,
}

#[allow(dead_code)]
fn parse_cb(cb: &[u8]) -> ScsiCommand {
    let Some(&opcode) = cb.first() else {
        return ScsiCommand::Unknown;
    };
    let required = match opcode {
        TEST_UNIT_READY | START_STOP_UNIT | INQUIRY | REQUEST_SENSE | MODE_SENSE_6 => 6,
        SYNCHRONIZE_CACHE_10
        | READ_CAPACITY_10
        | READ_10
        | WRITE_10
        | MODE_SENSE_10
        | READ_FORMAT_CAPACITIES => 10,
        READ_CAPACITY_16 | READ_16 => 16,
        _ => return ScsiCommand::Unknown,
    };
    if cb.len() < required {
        return ScsiCommand::Unknown;
    }
    if opcode == READ_CAPACITY_16 && cb[1] & 0x1F != 0x10 {
        return ScsiCommand::Unknown;
    }
    match opcode {
        TEST_UNIT_READY => ScsiCommand::TestUnitReady,
        START_STOP_UNIT => ScsiCommand::StartStop {
            start: cb[4] & 0x01 != 0,
            load_eject: cb[4] & 0x02 != 0,
        },
        SYNCHRONIZE_CACHE_10 => ScsiCommand::SynchronizeCache,
        INQUIRY => ScsiCommand::Inquiry {
            evpd: (cb[1] & 0b00000001) != 0,
            page_code: cb[2],
            alloc_len: u16::from_be_bytes([cb[3], cb[4]]),
        },
        REQUEST_SENSE => ScsiCommand::RequestSense {
            desc: (cb[1] & 0b00000001) != 0,
            alloc_len: cb[4],
        },
        READ_CAPACITY_10 => ScsiCommand::ReadCapacity10,
        READ_CAPACITY_16 => ScsiCommand::ReadCapacity16 {
            alloc_len: u32::from_be_bytes([cb[10], cb[11], cb[12], cb[13]]),
        },
        READ_10 => ScsiCommand::Read {
            lba: u32::from_be_bytes(cb[2..6].try_into().unwrap()) as u64,
            len: u16::from_be_bytes(cb[7..9].try_into().unwrap()) as u64,
        },
        READ_16 => ScsiCommand::Read {
            lba: u64::from_be_bytes((&cb[2..10]).try_into().unwrap()),
            len: u32::from_be_bytes((&cb[10..14]).try_into().unwrap()) as u64,
        },
        WRITE_10 => ScsiCommand::Write {
            lba: u32::from_be_bytes(cb[2..6].try_into().unwrap()) as u64,
            len: u16::from_be_bytes(cb[7..9].try_into().unwrap()) as u64,
        },
        MODE_SENSE_6 => ScsiCommand::ModeSense6 {
            dbd: (cb[1] & 0b00001000) != 0,
            page_control: PageControl::try_from_primitive(cb[2] >> 6).unwrap(),
            page_code: cb[2] & 0b00111111,
            subpage_code: cb[3],
            alloc_len: cb[4],
        },
        MODE_SENSE_10 => ScsiCommand::ModeSense10 {
            dbd: (cb[1] & 0b00001000) != 0,
            page_control: PageControl::try_from_primitive(cb[2] >> 6).unwrap(),
            page_code: cb[2] & 0b00111111,
            subpage_code: cb[3],
            alloc_len: u16::from_be_bytes([cb[7], cb[8]]),
        },
        READ_FORMAT_CAPACITIES => ScsiCommand::ReadFormatCapacities {
            alloc_len: u16::from_be_bytes([cb[7], cb[8]]),
        },
        _ => ScsiCommand::Unknown,
    }
}

/// SCSI USB Mass Storage subclass
pub struct Scsi<T: Transport> {
    interface: InterfaceNumber,
    pub(crate) transport: T,
}

/// SCSI subclass implementation with [Bulk Only Transport]
///
/// [Bulk Only Transport]: crate::transport::bbb::BulkOnly
#[cfg(feature = "bbb")]
impl<'alloc, Bus: UsbBus + 'alloc, Buf: BorrowMut<[u8]>> Scsi<BulkOnly<'alloc, Bus, Buf>> {
    /// Creates an SCSI over Bulk Only Transport instance
    ///
    /// # Arguments
    /// * `alloc` - [UsbBusAllocator]
    /// * `packet_size` - Maximum USB packet size. Allowed values: 8,16,32,64
    /// * `max_lun` - The max index of the Logical Unit
    /// * `buf` - The underlying IO buffer. It is **required** to fit at least a `CBW` and/or a single
    ///   packet. It is **recommended** that buffer fits at least one sector
    ///
    /// # Errors
    /// * [InvalidMaxLun]
    /// * [BufferTooSmall]
    ///
    /// # Panics
    /// Panics if endpoint allocation fails.
    ///
    /// [InvalidMaxLun]: crate::transport::bbb::BulkOnlyError::InvalidMaxLun
    /// [BufferTooSmall]: crate::transport::bbb::BulkOnlyError::BufferTooSmall
    /// [UsbBusAllocator]: usb_device::bus::UsbBusAllocator
    pub fn new(
        alloc: &'alloc UsbBusAllocator<Bus>,
        packet_size: u16,
        max_lun: u8,
        buf: Buf,
    ) -> Result<Self, BulkOnlyError> {
        BulkOnly::new(alloc, packet_size, max_lun, buf).map(|transport| Self {
            interface: alloc.interface(),
            transport,
        })
    }

    /// Drive BOT and handle the current Pager command once per poll.
    pub fn poll<F>(&mut self, mut callback: F) -> Result<(), UsbError>
    where
        F: FnMut(Command<ScsiCommand, Scsi<BulkOnly<'alloc, Bus, Buf>>>),
    {
        fn map_ignore(res: Result<(), TransportError<BulkOnlyError>>) -> Result<(), UsbError> {
            match res {
                Ok(())
                | Err(TransportError::Usb(UsbError::WouldBlock))
                | Err(TransportError::Error(_)) => Ok(()),
                Err(TransportError::Usb(err)) => Err(err),
            }
        }
        map_ignore(self.transport.poll())?;
        if !self.transport.has_status()
            && let Some(raw_cb) = self.transport.get_command()
        {
            let lun = raw_cb.lun;
            let kind = parse_cb(raw_cb.bytes);
            callback(Command {
                class: self,
                kind,
                lun,
            });
            map_ignore(self.transport.poll())?;
        }
        Ok(())
    }
}

impl<Bus, T> UsbClass<Bus> for Scsi<T>
where
    Bus: UsbBus,
    T: Transport<Bus = Bus>,
{
    fn get_configuration_descriptors(
        &self,
        writer: &mut DescriptorWriter,
    ) -> usb_device::Result<()> {
        writer.interface(self.interface, CLASS_MASS_STORAGE, SUBCLASS_SCSI, T::PROTO)?;

        self.transport.get_endpoint_descriptors(writer)?;

        Ok(())
    }

    fn reset(&mut self) {
        self.transport.reset()
    }

    fn control_in(&mut self, xfer: ControlIn<Bus>) {
        self.transport.control_in(xfer)
    }

    fn poll(&mut self) {
        if let Err(err) = self.transport.poll() {
            trace!("usb: scsi: poll: {}", err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_lba_and_count_remain_big_endian() {
        for opcode in [READ_10, WRITE_10] {
            let mut command = [0; 10];
            command[0] = opcode;
            command[2..6].copy_from_slice(&65536u32.to_be_bytes());
            command[7..9].copy_from_slice(&4096u16.to_be_bytes());
            match parse_cb(&command) {
                ScsiCommand::Read { lba, len } | ScsiCommand::Write { lba, len } => {
                    assert_eq!(lba, 65536);
                    assert_eq!(len, 4096);
                }
                _ => panic!("valid command refused"),
            }
            command[2..6].copy_from_slice(&u32::MAX.to_be_bytes());
            command[7..9].copy_from_slice(&u16::MAX.to_be_bytes());
            assert!(matches!(
                parse_cb(&command),
                ScsiCommand::Read {
                    lba: 4294967295,
                    len: 65535
                } | ScsiCommand::Write {
                    lba: 4294967295,
                    len: 65535
                }
            ));
        }
    }
    #[test]
    fn every_truncated_cdb_is_rejected_without_indexing() {
        assert!(matches!(parse_cb(&[]), ScsiCommand::Unknown));
        for (opcode, length) in [
            (READ_10, 10),
            (WRITE_10, 10),
            (READ_16, 16),
            (READ_CAPACITY_16, 16),
            (INQUIRY, 6),
            (MODE_SENSE_6, 6),
            (MODE_SENSE_10, 10),
            (START_STOP_UNIT, 6),
        ] {
            let mut command = [0; 16];
            command[0] = opcode;
            for truncated in 1..length {
                assert!(matches!(
                    parse_cb(&command[..truncated]),
                    ScsiCommand::Unknown
                ));
            }
        }
    }
}
