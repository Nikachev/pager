//! CDC-ACM USB Serial logger and command reception task.

use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{Receiver, Sender};
use embedded_io_async::Write;

use crate::{NrfUsbDriver, LOG_CHANNEL};

#[embassy_executor::task]
pub async fn usb_logger_task(mut sender: Sender<'static, NrfUsbDriver>) -> ! {
    let _ = sender.write_all(b"Pager serial logger started.\r\n").await;
    loop {
        let msg = LOG_CHANNEL.receive().await;
        if sender.write_all(msg.as_bytes()).await.is_err() {
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }
        let _ = sender.write_all(b"\r\n").await;
    }
}

#[embassy_executor::task]
pub async fn usb_receiver_task(mut receiver: Receiver<'static, NrfUsbDriver>) -> ! {
    // CDC is diagnostics-only. Drain OUT packets so hosts never see a stalled
    // endpoint; all state-changing commands use the versioned WebUSB protocol.
    loop {
        let mut buf = [0u8; 64];
        match receiver.read_packet(&mut buf).await {
            Ok(_) => {}
            _ => {
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }
}
