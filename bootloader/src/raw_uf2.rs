//! Raw Vendor-Specific (0xFF) USB Endpoint Handler for UF2 Flashing
//!
//! Provides 100% unrestricted USB Bulk transfer endpoints (0x01 OUT, 0x81 IN)
//! that bypass macOS/Linux/Windows kernel driver locks.

use usb_device::bus::{InterfaceNumber, UsbBus, UsbBusAllocator};
use usb_device::descriptor::DescriptorWriter;
use usb_device::endpoint::{EndpointIn, EndpointOut};
use usb_device::UsbError;

pub struct RawUf2Class<'a, B: UsbBus> {
    interface: InterfaceNumber,
    ep_out: EndpointOut<'a, B>,
    ep_in: EndpointIn<'a, B>,
}

impl<'a, B: UsbBus> RawUf2Class<'a, B> {
    pub fn new(alloc: &'a UsbBusAllocator<B>) -> Self {
        let interface = alloc.interface();
        let ep_out = alloc.bulk(64);
        let ep_in = alloc.bulk(64);

        Self {
            interface,
            ep_out,
            ep_in,
        }
    }

    pub fn read_packet(&mut self, buf: &mut [u8]) -> Result<usize, UsbError> {
        self.ep_out.read(buf)
    }

    pub fn write_packet(&mut self, buf: &[u8]) -> Result<usize, UsbError> {
        self.ep_in.write(buf)
    }
}

impl<B: UsbBus> usb_device::class::UsbClass<B> for RawUf2Class<'_, B> {
    fn get_configuration_descriptors(
        &self,
        writer: &mut DescriptorWriter,
    ) -> Result<(), UsbError> {
        writer.interface(self.interface, 0xFF, 0x00, 0x00)?; // Vendor-Specific Interface (0xFF)
        writer.endpoint(&self.ep_out)?;
        writer.endpoint(&self.ep_in)?;
        Ok(())
    }
}
