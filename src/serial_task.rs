//! CDC-ACM USB Serial logger and command reception task.

use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{Receiver, Sender};
use embedded_io_async::Write;

use crate::{ble, NrfUsbDriver, LOG_CHANNEL};

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
    let mut cmd_buf = [0u8; 128];
    let mut cmd_len = 0;

    loop {
        let mut buf = [0u8; 64];
        match receiver.read_packet(&mut buf).await {
            Ok(n) if n > 0 => {
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if cmd_len > 0 {
                            let cmd_str = core::str::from_utf8(&cmd_buf[..cmd_len])
                                .unwrap_or("")
                                .trim();
                            cmd_len = 0;

                            if cmd_str == "dfu" || cmd_str == "reboot" || cmd_str == "bootloader" {
                                crate::log_msg!("SERIAL:REBOOT_TO_BOOTLOADER");
                                Timer::after(Duration::from_millis(100)).await;
                                crate::flash::enter_bootloader();
                            } else if let Some(text) = cmd_str.strip_prefix("type ") {
                                crate::log_msg!("Serial command type: {}", text);
                                let mut s = heapless::String::<128>::new();
                                let _ = s.push_str(text);
                                if !ble::try_send_command(ble::BleCommand::TypeString(s)) {
                                    crate::log_msg!(
                                        "BLE command queue full during serial type request"
                                    );
                                }
                            } else if cmd_str == "pair" {
                                crate::log_msg!("Serial command: Triggering pairing mode");
                                if !ble::try_send_command(ble::BleCommand::RestartAdvertising) {
                                    crate::log_msg!(
                                        "BLE command queue full during serial pairing request"
                                    );
                                }
                            } else if cmd_str == "ping" {
                                crate::log_msg!("SERIAL:PONG");
                            } else if cmd_str == "clear_bonds" {
                                crate::log_msg!("Serial command: Clearing persistent bonds");
                                ble::KEYBOARD_STATE.lock(|state| {
                                    let mut state = state.borrow_mut();
                                    state.bonds = [None, None, None];
                                });
                                ble::PERSIST_STATE.signal(());
                                let _ = ble::try_send_command(ble::BleCommand::SyncActiveBond);
                            }
                        }
                    } else {
                        if cmd_len >= cmd_buf.len() {
                            cmd_len = 0;
                        }
                        cmd_buf[cmd_len] = b;
                        cmd_len += 1;
                    }
                }
            }
            _ => {
                Timer::after(Duration::from_millis(10)).await;
            }
        }
    }
}
