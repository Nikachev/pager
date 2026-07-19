use defmt::*;
use nrf_softdevice::ble::advertisement_builder::{
    Flag, LegacyAdvertisementBuilder, ServiceList,
};
use nrf_softdevice::ble::{gatt_server, peripheral};
use nrf_softdevice::Softdevice;

// Custom 128-bit Base UUID: 9e7a0000-0b3e-46e8-ad30-7746bad7128a
// Service UUID: 9e7a0001-0b3e-46e8-ad30-7746bad7128a
// LED Characteristic UUID: 9e7a0002-0b3e-46e8-ad30-7746bad7128a
// Status Characteristic UUID: 9e7a0003-0b3e-46e8-ad30-7746bad7128a

#[nrf_softdevice::gatt_service(uuid = "9e7a0001-0b3e-46e8-ad30-7746bad7128a")]
pub struct CustomService {
    #[characteristic(uuid = "9e7a0002-0b3e-46e8-ad30-7746bad7128a", write)]
    pub led: u8,

    #[characteristic(uuid = "9e7a0003-0b3e-46e8-ad30-7746bad7128a", read, notify)]
    pub status: u8,
}

#[nrf_softdevice::gatt_server]
pub struct Server {
    pub custom: CustomService,
}

#[embassy_executor::task]
pub async fn ble_task(sd: &'static Softdevice, server: &'static Server) -> ! {
    crate::log_msg!("BLE GATT Server task started.");

    let adv_payload = LegacyAdvertisementBuilder::new()
        .flags(&[Flag::GeneralDiscovery, Flag::LE_Only])
        .services_128(
            ServiceList::Complete,
            &[[
                0x8a, 0x12, 0xd7, 0xba, 0x46, 0x77, 0x30, 0xad,
                0xe8, 0x46, 0x3e, 0x0b, 0x01, 0x00, 0x7a, 0x9e,
            ]],
        )
        .build();

    let scan_payload = LegacyAdvertisementBuilder::new()
        .short_name("nice_nano")
        .build();

    loop {
        let config = peripheral::Config::default();
        crate::log_msg!("BLE Advertising started...");
        let conn = match peripheral::advertise_connectable(
            sd,
            peripheral::ConnectableAdvertisement::ScannableUndirected {
                adv_data: &adv_payload,
                scan_data: &scan_payload,
            },
            &config,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!("BLE advertising failed: {:?}", e);
                continue;
            }
        };

        crate::log_msg!("BLE Connection established from {:?}", conn.peer_address());

        // Periodically notify client with system uptime
        let server_ref = server;
        let conn_ref = &conn;
        let notify_task = async move {
            let mut val = 0u8;
            loop {
                embassy_time::Timer::after(embassy_time::Duration::from_secs(5)).await;
                val = val.wrapping_add(1);
                if let Err(e) = server_ref.custom.status_notify(conn_ref, &val) {
                    warn!("BLE notify error: {:?}", e);
                    break;
                }
            }
        };

        let gatt_task = async {
            let e = gatt_server::run(&conn, server, |e| match e {
                ServerEvent::Custom(e) => match e {
                    CustomServiceEvent::LedWrite(val) => {
                        crate::log_msg!("BLE LED command received: {}", val);
                        crate::LED_MODE.signal(val);
                    }
                    CustomServiceEvent::StatusCccdWrite { notifications } => {
                        crate::log_msg!("BLE status notification configuration changed: {}", notifications);
                    }
                },
            })
            .await;
            crate::log_msg!("BLE connection closed; error: {:?}", e);
        };

        // Run both notify loop and gatt server loop concurrently
        embassy_futures::select::select(notify_task, gatt_task).await;
    }
}
