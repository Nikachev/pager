//! CDC-ACM USB Serial logger and command reception task.

use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{Receiver, Sender};
use embedded_io_async::Write;
use nrf_mpsl::Flash;

use crate::{ble, web, MyDriver, LOG_CHANNEL};

#[embassy_executor::task]
pub async fn usb_logger_task(mut sender: Sender<'static, MyDriver>) -> ! {
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
pub async fn usb_receiver_task(
    mut receiver: Receiver<'static, MyDriver>,
    flash_mutex: &'static Mutex<ThreadModeRawMutex, Flash<'static>>,
) -> ! {
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

                            if cmd_str == "dfu" || cmd_str == "reboot" {
                                crate::log_msg!("Rebooting device...");
                                Timer::after(Duration::from_millis(100)).await;
                                cortex_m::peripheral::SCB::sys_reset();
                            } else if let Some(stripped) = cmd_str.strip_prefix("update") {
                                let mut fields = stripped.split_whitespace();
                                let content_len: usize =
                                    match fields.next().and_then(|s| s.parse().ok()) {
                                        Some(l) if l > 0 && l <= web::MAX_BIN_SIZE => l,
                                        _ => {
                                            crate::log_msg!("SERIAL_UPDATE:ERROR_INVALID_SIZE");
                                            continue;
                                        }
                                    };
                                let expected_crc = match fields.next().and_then(|s| {
                                    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
                                }) {
                                    Some(crc) if fields.next().is_none() => crc,
                                    _ => {
                                        crate::log_msg!("SERIAL_UPDATE:ERROR_INVALID_CHECKSUM");
                                        continue;
                                    }
                                };
                                crate::flash::stop_flashing();
                                if !crate::flash::try_start_flashing() {
                                    crate::log_msg!("SERIAL_UPDATE:REJECTED_DFU_IN_PROGRESS");
                                    continue;
                                }
                                crate::log_msg!("SERIAL_UPDATE:START:{}", content_len);
                                crate::log_msg!(
                                    "SERIAL_UPDATE:READY:{}:{:08x}",
                                    content_len,
                                    expected_crc
                                );
                                let mut flash = flash_mutex.lock().await;
                                crate::log_msg!("SERIAL_DFU_MUTEX_LOCKED");
                                let mut writer = crate::flash::OtaWriter::new(
                                    &mut *flash,
                                    web::inactive_slot_manifest_addr(),
                                    web::inactive_slot_manifest_addr() + web::MAX_BIN_SIZE as u32,
                                );

                                let mut total_read = 0;
                                let mut crc = crate::flash::CRC32_INIT;
                                let mut write_failed = false;
                                let mut chunk_buf = [0u8; 64];
                                let mut idle_count = 0;
                                while total_read < content_len {
                                    match receiver.read_packet(&mut chunk_buf).await {
                                        Ok(cn) if cn > 0 => {
                                            idle_count = 0;
                                            crate::signal_heartbeat(crate::HEARTBEAT_BLINK);
                                            let to_write =
                                                core::cmp::min(cn, content_len - total_read);
                                            if let Err(e) =
                                                writer.write_chunk(&chunk_buf[..to_write]).await
                                            {
                                                crate::log_msg!(
                                                    "SERIAL_UPDATE:FLASH_ERROR:{:?}",
                                                    e
                                                );
                                                write_failed = true;
                                                break;
                                            }
                                            crc = crate::flash::crc32_update(
                                                crc,
                                                &chunk_buf[..to_write],
                                            );
                                            total_read += to_write;
                                        }
                                        _ => {
                                            idle_count += 1;
                                            if idle_count > 1000 {
                                                crate::log_msg!("SERIAL_UPDATE:TIMEOUT");
                                                crate::flash::stop_flashing();
                                                break;
                                            }
                                            Timer::after(Duration::from_millis(1)).await;
                                        }
                                    }
                                }
                                if !write_failed
                                    && total_read == content_len
                                    && crate::flash::crc32_finalize(crc) == expected_crc
                                {
                                    if let Err(e) = writer.flush().await {
                                        crate::log_msg!("SERIAL_UPDATE:FLASH_ERROR:{:?}", e);
                                        crate::flash::stop_flashing();
                                        continue;
                                    }
                                    let _ = writer;
                                    match crate::flash::package_targets_slot(
                                        &mut *flash,
                                        web::inactive_slot_manifest_addr(),
                                        content_len,
                                        web::inactive_slot(),
                                    )
                                    .await
                                    {
                                        Ok(true) => {}
                                        Ok(false) => {
                                            crate::log_msg!("SERIAL_UPDATE:ERROR_TARGET_SLOT");
                                            crate::flash::stop_flashing();
                                            continue;
                                        }
                                        Err(e) => {
                                            crate::log_msg!("SERIAL_UPDATE:FLASH_ERROR:{:?}", e);
                                            crate::flash::stop_flashing();
                                            continue;
                                        }
                                    }
                                    crate::log_msg!("SERIAL_UPDATE:COMPLETE");
                                    Timer::after(Duration::from_millis(1500)).await;
                                    crate::flash::stop_flashing();
                                    web::reset_after_usb_detach().await;
                                } else {
                                    if !write_failed && total_read == content_len {
                                        crate::log_msg!("SERIAL_UPDATE:CHECKSUM_MISMATCH");
                                    }
                                    crate::flash::stop_flashing();
                                }
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
