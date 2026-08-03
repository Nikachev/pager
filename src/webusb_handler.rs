//! WebUSB control plane task and frame handlers.

use crate::protocol;
use embassy_time::{Duration, Timer};

use crate::{ble, web, webusb, MyDriver};

const USB_COMMAND_PING: u8 = 1;
const USB_COMMAND_GET_INFO: u8 = 2;
const USB_COMMAND_GET_KEYBOARD_STATE: u8 = 3;
const USB_COMMAND_SWITCH_PROFILE: u8 = 4;
const USB_COMMAND_ENABLE_PAIRING: u8 = 5;
const USB_COMMAND_DISCONNECT: u8 = 6;
const USB_COMMAND_CLEAR_PROFILE: u8 = 7;
const USB_COMMAND_TYPE_TEXT: u8 = 8;
const USB_COMMAND_DFU_BEGIN: u8 = 9;
const USB_COMMAND_DFU_COMMIT: u8 = 10;
const USB_COMMAND_DFU_ABORT: u8 = 11;
const USB_ERROR_BAD_REQUEST: u8 = 1;
const USB_ERROR_UNSUPPORTED_COMMAND: u8 = 2;
const USB_ERROR_BUSY: u8 = 3;
const USB_ERROR_DFU: u8 = 4;

pub async fn webusb_reply(
    transport: &mut webusb::Transport<'static, MyDriver>,
    kind: protocol::UsbFrameKind,
    request_id: u32,
    payload: &[u8],
) {
    let mut frame = [0u8; protocol::USB_FRAME_HEADER_LEN + protocol::USB_MAX_PAYLOAD];
    if let Ok(n) = protocol::encode_usb_frame(&mut frame, kind, request_id, payload) {
        let _ = transport.write_frame(&frame[..n]).await;
    }
}

#[embassy_executor::task]
pub async fn webusb_task(mut transport: webusb::Transport<'static, MyDriver>) -> ! {
    let mut pending = [0u8; protocol::USB_FRAME_HEADER_LEN + protocol::USB_MAX_PAYLOAD];
    let mut pending_len: usize;
    let mut transfer = [0u8; 64];

    loop {
        transport.wait_connection().await;
        crate::flash::stop_flashing();
        pending_len = 0;
        let mut dfu_len = 0usize;
        let mut dfu_offset = 0usize;
        crate::log_msg!("WEBUSB:CONNECTED");

        loop {
            let n = match transport.read_transfer(&mut transfer).await {
                Ok(n) if n > 0 => n,
                _ => {
                    crate::flash::stop_flashing();
                    break;
                }
            };
            if pending_len + n > pending.len() {
                pending_len = 0;
                webusb_reply(
                    &mut transport,
                    protocol::UsbFrameKind::Error,
                    0,
                    &[USB_ERROR_BAD_REQUEST],
                )
                .await;
                continue;
            }
            pending[pending_len..pending_len + n].copy_from_slice(&transfer[..n]);
            pending_len += n;

            loop {
                if pending_len < protocol::USB_FRAME_HEADER_LEN {
                    break;
                }
                let declared = u16::from_le_bytes([pending[10], pending[11]]) as usize;
                if declared > protocol::USB_MAX_PAYLOAD {
                    pending_len = 0;
                    webusb_reply(
                        &mut transport,
                        protocol::UsbFrameKind::Error,
                        0,
                        &[USB_ERROR_BAD_REQUEST],
                    )
                    .await;
                    break;
                }
                let frame_len = protocol::USB_FRAME_HEADER_LEN + declared;
                if pending_len < frame_len {
                    break;
                }

                match protocol::parse_usb_frame(&pending[..frame_len]) {
                    Ok((header, payload)) if header.kind == protocol::UsbFrameKind::Command => {
                        if dfu_len > 0
                            && !matches!(
                                payload,
                                [USB_COMMAND_DFU_COMMIT] | [USB_COMMAND_DFU_ABORT]
                            )
                        {
                            web::OTA_CANCEL_SIGNAL.signal(());
                            crate::flash::stop_flashing();
                            dfu_len = 0;
                            dfu_offset = 0;
                        }
                        match payload {
                            [USB_COMMAND_PING] => {
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    b"PONG",
                                )
                                .await;
                            }
                            [USB_COMMAND_GET_INFO] => {
                                let info: &[u8] = if web::running_slot() == 0 {
                                    b"Pager;protocol=1;slot=A"
                                } else {
                                    b"Pager;protocol=1;slot=B"
                                };
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    info,
                                )
                                .await;
                            }
                            [USB_COMMAND_GET_KEYBOARD_STATE] => {
                                let state = ble::KEYBOARD_STATE.lock(|state| {
                                    let state = state.borrow();
                                    [
                                        state.active_slot as u8,
                                        state.pairing_mode as u8,
                                        state.bonds[0].is_some() as u8,
                                        state.bonds[1].is_some() as u8,
                                        state.bonds[2].is_some() as u8,
                                    ]
                                });
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    &state,
                                )
                                .await;
                            }
                            [USB_COMMAND_SWITCH_PROFILE, slot @ 0..=2] => {
                                ble::KEYBOARD_STATE.lock(|state| {
                                    let mut state = state.borrow_mut();
                                    state.active_slot = *slot as usize;
                                    state.pairing_mode = false;
                                });
                                ble::PERSIST_STATE.signal(());
                                let result =
                                    if ble::try_send_command(ble::BleCommand::RestartAdvertising) {
                                        protocol::UsbFrameKind::Response
                                    } else {
                                        protocol::UsbFrameKind::Error
                                    };
                                let body = if result == protocol::UsbFrameKind::Response {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BUSY][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_ENABLE_PAIRING] => {
                                ble::KEYBOARD_STATE.lock(|state| {
                                    let mut state = state.borrow_mut();
                                    let active_slot = state.active_slot;
                                    state.bonds[active_slot] = None;
                                    state.pairing_mode = true;
                                });
                                ble::PERSIST_STATE.signal(());
                                let result =
                                    if ble::try_send_command(ble::BleCommand::RestartAdvertising) {
                                        protocol::UsbFrameKind::Response
                                    } else {
                                        protocol::UsbFrameKind::Error
                                    };
                                let body = if result == protocol::UsbFrameKind::Response {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BUSY][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_DISCONNECT] => {
                                let result = if ble::try_send_command(ble::BleCommand::Disconnect) {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                };
                                let body = if result == protocol::UsbFrameKind::Response {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BUSY][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_CLEAR_PROFILE, slot @ 0..=2] => {
                                ble::KEYBOARD_STATE
                                    .lock(|state| state.borrow_mut().bonds[*slot as usize] = None);
                                ble::PERSIST_STATE.signal(());
                                let result =
                                    if ble::try_send_command(ble::BleCommand::RestartAdvertising) {
                                        protocol::UsbFrameKind::Response
                                    } else {
                                        protocol::UsbFrameKind::Error
                                    };
                                let body = if result == protocol::UsbFrameKind::Response {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BUSY][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_TYPE_TEXT, text @ ..] => {
                                let command = core::str::from_utf8(text)
                                    .ok()
                                    .and_then(|text| heapless::String::<128>::try_from(text).ok())
                                    .map(ble::BleCommand::TypeString);
                                let result = match command {
                                    Some(command) => {
                                        if ble::try_send_command(command) {
                                            protocol::UsbFrameKind::Response
                                        } else {
                                            protocol::UsbFrameKind::Error
                                        }
                                    }
                                    None => protocol::UsbFrameKind::Error,
                                };
                                let body = if result == protocol::UsbFrameKind::Response {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BUSY][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_DFU_BEGIN, size @ ..] if size.len() == 8 => {
                                let content_len =
                                    u32::from_le_bytes(size[..4].try_into().unwrap()) as usize;
                                let expected_crc =
                                    u32::from_le_bytes(size[4..8].try_into().unwrap());
                                crate::log_msg!(
                                    "WEBUSB:DFU_BEGIN len={} crc={:x}",
                                    content_len,
                                    expected_crc
                                );
                                if dfu_len == 0 {
                                    crate::flash::stop_flashing();
                                }
                                if content_len == 0 || content_len > web::MAX_BIN_SIZE {
                                    crate::log_msg!("WEBUSB:DFU_BEGIN_REJECTED_BAD_LEN");
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[USB_ERROR_DFU],
                                    )
                                    .await;
                                } else if !crate::flash::try_start_flashing() {
                                    crate::log_msg!("WEBUSB:DFU_BEGIN_REJECTED_FLASHING_ACTIVE");
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[USB_ERROR_DFU],
                                    )
                                    .await;
                                } else {
                                    web::OTA_READY_SIGNAL.reset();
                                    web::OTA_RESULT_SIGNAL.reset();
                                    web::OTA_COMMAND_SIGNAL.signal(web::OtaCommand::Start {
                                        content_len,
                                        expected_crc,
                                        target_start: web::inactive_slot_manifest_addr(),
                                        target_slot: web::inactive_slot(),
                                    });
                                    if web::OTA_READY_SIGNAL.wait().await.is_ok() {
                                        dfu_len = content_len;
                                        dfu_offset = 0;
                                        webusb_reply(
                                            &mut transport,
                                            protocol::UsbFrameKind::Response,
                                            header.request_id,
                                            &0u32.to_le_bytes(),
                                        )
                                        .await;
                                    } else {
                                        crate::log_msg!(
                                            "WEBUSB:DFU_BEGIN_REJECTED_OTA_READY_FAILED"
                                        );
                                        crate::flash::stop_flashing();
                                        webusb_reply(
                                            &mut transport,
                                            protocol::UsbFrameKind::Error,
                                            header.request_id,
                                            &[USB_ERROR_DFU],
                                        )
                                        .await;
                                    }
                                }
                            }
                            [USB_COMMAND_DFU_COMMIT] if dfu_len > 0 && dfu_offset == dfu_len => {
                                if web::OTA_RESULT_SIGNAL.wait().await.is_ok() {
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Response,
                                        header.request_id,
                                        &[0],
                                    )
                                    .await;
                                    Timer::after(Duration::from_millis(1500)).await;
                                    web::reset_after_usb_detach().await;
                                } else {
                                    dfu_len = 0;
                                    dfu_offset = 0;
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[USB_ERROR_DFU],
                                    )
                                    .await;
                                }
                            }
                            [USB_COMMAND_DFU_ABORT] if dfu_len > 0 => {
                                web::OTA_CANCEL_SIGNAL.signal(());
                                crate::flash::stop_flashing();
                                dfu_len = 0;
                                dfu_offset = 0;
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    &[0],
                                )
                                .await;
                            }
                            _ => {
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Error,
                                    header.request_id,
                                    &[USB_ERROR_UNSUPPORTED_COMMAND],
                                )
                                .await;
                            }
                        }
                    }
                    Ok((header, payload))
                        if header.kind == protocol::UsbFrameKind::DfuData
                            && dfu_len > 0
                            && payload.len() >= 5 =>
                    {
                        let offset = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
                        let bytes = &payload[4..];
                        if offset + bytes.len() <= dfu_offset {
                            webusb_reply(
                                &mut transport,
                                protocol::UsbFrameKind::Response,
                                header.request_id,
                                &(dfu_offset as u32).to_le_bytes(),
                            )
                            .await;
                        } else if offset != dfu_offset || bytes.len() > dfu_len - dfu_offset {
                            webusb_reply(
                                &mut transport,
                                protocol::UsbFrameKind::Error,
                                header.request_id,
                                &[USB_ERROR_DFU],
                            )
                            .await;
                        } else {
                            match web::OTA_PIPE.try_write(bytes) {
                                Ok(written) if written > 0 => {
                                    dfu_offset += written;
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Response,
                                        header.request_id,
                                        &(dfu_offset as u32).to_le_bytes(),
                                    )
                                    .await;
                                }
                                _ => {
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[USB_ERROR_BUSY],
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    _ => {
                        webusb_reply(
                            &mut transport,
                            protocol::UsbFrameKind::Error,
                            0,
                            &[USB_ERROR_BAD_REQUEST],
                        )
                        .await;
                    }
                }
                let remaining = pending_len - frame_len;
                pending.copy_within(frame_len..pending_len, 0);
                pending_len = remaining;
            }
        }
        crate::log_msg!("WEBUSB:DISCONNECTED");
    }
}
