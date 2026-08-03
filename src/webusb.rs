//! WebUSB landing-page capability and Pager protocol bulk transport.

use embassy_usb::control::{InResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Driver, Endpoint, EndpointError, EndpointIn, EndpointOut};
use embassy_usb::{Builder, Handler};

const VENDOR_CLASS: u8 = 0xff;
const WEBUSB_VENDOR_CODE: u8 = 0x22;
const WEBUSB_GET_URL: u16 = 0x02;
const WEBUSB_URL_DESCRIPTOR: u8 = 0x03;
const LANDING_PAGE: &[u8] = b"github.com/nikachev/pager";

/// Handles the vendor request defined by the WebUSB BOS capability.
pub struct LandingPageControl {
    response: [u8; 128],
}

impl LandingPageControl {
    pub const fn new() -> Self {
        Self { response: [0; 128] }
    }
}

impl Handler for LandingPageControl {
    fn control_in(&mut self, req: Request, _data: &mut [u8]) -> Option<InResponse<'_>> {
        if req.request_type != RequestType::Vendor
            || req.recipient != Recipient::Device
            || req.request != WEBUSB_VENDOR_CODE
            || req.value != 1
            || req.index != WEBUSB_GET_URL
        {
            return None;
        }
        self.response[0] = (LANDING_PAGE.len() + 3) as u8;
        self.response[1] = WEBUSB_URL_DESCRIPTOR;
        self.response[2] = 1; // https://
        self.response[3..3 + LANDING_PAGE.len()].copy_from_slice(LANDING_PAGE);
        Some(InResponse::Accepted(
            &self.response[..3 + LANDING_PAGE.len()],
        ))
    }
}

/// Pager's vendor-specific bulk interface. This intentionally does not use
/// CDC-ACM: browsers can claim this interface without interacting with an OS
/// serial driver.
pub struct Transport<'d, D: Driver<'d>> {
    read_ep: D::EndpointOut,
    write_ep: D::EndpointIn,
    max_packet_size: usize,
}

impl<'d, D: Driver<'d>> Transport<'d, D> {
    pub fn new(
        builder: &mut Builder<'d, D>,
        control: &'d mut LandingPageControl,
        max_packet_size: u16,
    ) -> Self {
        let mut function = builder.function(VENDOR_CLASS, 0, 0);
        let mut interface = function.interface();
        let mut alt = interface.alt_setting(VENDOR_CLASS, 0, 0, None);
        alt.bos_capability(
            embassy_usb::descriptor::capability_type::PLATFORM,
            &[
                // WebUSB platform capability UUID, encoded in USB byte order.
                0x00,
                0x38,
                0xb6,
                0x08,
                0x34,
                0xa9,
                0x09,
                0xa0,
                0x47,
                0x8b,
                0xfd,
                0xa0,
                0x76,
                0x88,
                0x15,
                0xb6,
                0x65,
                0x00,
                0x01, // WebUSB 1.0
                WEBUSB_VENDOR_CODE,
                1, // landing page index
            ],
        );
        let read_ep = alt.endpoint_bulk_out(None, max_packet_size);
        let write_ep = alt.endpoint_bulk_in(None, max_packet_size);
        drop(function);
        builder.handler(control);
        Self {
            read_ep,
            write_ep,
            max_packet_size: max_packet_size as usize,
        }
    }

    pub async fn wait_connection(&mut self) {
        self.read_ep.wait_enabled().await;
    }

    /// Reads one USB transfer. The framing layer handles transfer boundaries.
    pub async fn read_transfer(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        self.read_ep.read(buf).await
    }

    /// Writes a complete protocol frame, splitting it into full-speed packets.
    pub async fn write_frame(&mut self, data: &[u8]) -> Result<(), EndpointError> {
        for chunk in data.chunks(self.max_packet_size) {
            self.write_ep.write(chunk).await?;
        }
        if data.len().is_multiple_of(self.max_packet_size) {
            self.write_ep.write(&[]).await?;
        }
        Ok(())
    }
}
