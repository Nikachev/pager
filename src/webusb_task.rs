//! WebUSB control plane task and frame handlers.

use crate::protocol;
use embassy_futures::select::{select, Either};
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Timer};

use crate::{ble, usb_detach, webusb, NrfUsbDriver};

use protocol::spec::{
    ACTIVATE_SLOT as USB_COMMAND_ACTIVATE_SLOT, CANCEL_PAIRING as USB_COMMAND_CANCEL_PAIRING,
    CLEAR_SLOT as USB_COMMAND_CLEAR_SLOT, FACTORY_RESET as USB_COMMAND_FACTORY_RESET,
    GET_INFO as USB_COMMAND_GET_INFO, GET_LOGS as USB_COMMAND_GET_LOGS,
    GET_STATE as USB_COMMAND_GET_STATE, PING as USB_COMMAND_PING,
    REBOOT_TO_BOOTLOADER as USB_COMMAND_REBOOT_BOOTLOADER,
    SET_BLUETOOTH_ENABLED as USB_COMMAND_SET_BLUETOOTH_ENABLED,
    SET_DEVICE_NAME as USB_COMMAND_SET_DEVICE_NAME, SET_SLOT_NAME as USB_COMMAND_SET_SLOT_NAME,
    TYPE_TEXT as USB_COMMAND_TYPE_TEXT,
};

const USB_ERROR_BAD_REQUEST: u8 = protocol::spec::ERROR_BAD_REQUEST;
const USB_ERROR_UNSUPPORTED_COMMAND: u8 = protocol::spec::ERROR_UNSUPPORTED_COMMAND;
const USB_ERROR_BUSY: u8 = protocol::spec::ERROR_BUSY;

async fn run_ble_control_result(command: ble::BleCommand) -> u8 {
    ble::BLE_CONTROL_RESULT.reset();
    if !ble::try_send_command(command) {
        return protocol::spec::ERROR_QUEUE_FULL;
    }
    match select(
        ble::BLE_CONTROL_RESULT.wait(),
        Timer::after(Duration::from_secs(10)),
    )
    .await
    {
        Either::First(0) => 0,
        Either::First(2) => protocol::spec::ERROR_UNSUPPORTED_CHARACTER,
        Either::First(3) => protocol::spec::ERROR_CONNECTION_LOST,
        Either::First(_) | Either::Second(_) => USB_ERROR_BUSY,
    }
}

async fn run_ble_control(command: ble::BleCommand) -> bool {
    run_ble_control_result(command).await == 0
}

async fn wait_persist(sequence: u32) -> bool {
    loop {
        match select(
            ble::PERSIST_DONE.wait(),
            Timer::after(Duration::from_secs(5)),
        )
        .await
        {
            Either::First((completed, result))
                if completed.wrapping_sub(sequence) < 0x8000_0000 =>
            {
                return result;
            }
            Either::First(_) => continue,
            Either::Second(_) => return false,
        }
    }
}

pub async fn webusb_reply(
    transport: &mut webusb::Transport<'static, NrfUsbDriver>,
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
pub async fn webusb_task(mut transport: webusb::Transport<'static, NrfUsbDriver>) -> ! {
    let mut pending = [0u8; protocol::USB_FRAME_HEADER_LEN + protocol::USB_MAX_PAYLOAD];
    let mut pending_len: usize;
    let mut transfer = [0u8; 64];
    let mut events = match ble::STATE_EVENTS.subscriber() {
        Ok(events) => events,
        Err(_) => {
            crate::log_msg!("WEBUSB:STATE_SUBSCRIBER_UNAVAILABLE");
            loop {
                crate::signal_heartbeat(crate::HEARTBEAT_WEBUSB);
                Timer::after(Duration::from_secs(1)).await;
            }
        }
    };

    loop {
        transport.wait_connection().await;
        pending_len = 0;
        crate::log_msg!("WEBUSB:CONNECTED");

        loop {
            crate::signal_heartbeat(crate::HEARTBEAT_WEBUSB);
            let n = match select(
                transport.read_transfer(&mut transfer),
                events.next_message(),
            )
            .await
            {
                Either::First(Ok(n)) if n > 0 => n,
                Either::First(_) => break,
                Either::Second(event) => {
                    let mut payload = [0u8; 5];
                    match event {
                        WaitResult::Message(revision) => {
                            payload[0] = 1; // state changed
                            payload[1..5].copy_from_slice(&revision.to_le_bytes());
                        }
                        WaitResult::Lagged(_) => payload[0] = 0xFF, // snapshot required
                    }
                    webusb_reply(&mut transport, protocol::UsbFrameKind::Event, 0, &payload).await;
                    continue;
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
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    concat!(
                                        "Pager;protocol=5;bootloader=single-slot;version=",
                                        env!("PAGER_BUILD_VERSION")
                                    )
                                    .as_bytes(),
                                )
                                .await;
                            }
                            [USB_COMMAND_GET_STATE] => {
                                let mut payload = heapless::Vec::<u8, 256>::new();
                                ble::KEYBOARD_STATE.lock(|state| {
                                    let state = state.borrow();
                                    let _ = payload.push(protocol::spec::STATE_SCHEMA);
                                    let _ = payload.push(state.bluetooth_enabled as u8);
                                    let _ = payload.push(state.link_state as u8);
                                    let _ = payload.push(
                                        state.active_profile.map(|slot| slot as u8).unwrap_or(0xFF),
                                    );
                                    let _ = payload.push(
                                        state
                                            .connected_profile
                                            .map(|slot| slot as u8)
                                            .unwrap_or(0xFF),
                                    );
                                    let _ = payload.push(state.pairing_mode as u8);
                                    let _ = payload.push(state.bonds[0].is_some() as u8);
                                    let _ = payload.push(state.bonds[1].is_some() as u8);
                                    let _ = payload.push(state.bonds[2].is_some() as u8);
                                    let _ = payload.push(state.hid_ready as u8);

                                    for i in 0..3 {
                                        let name = if state.bonds[i].is_some() {
                                            state.slot_names[i].clone()
                                        } else {
                                            heapless::String::new()
                                        };
                                        let name_bytes = name.as_bytes();
                                        let len = name_bytes.len().min(64) as u8;
                                        let _ = payload.push(len);
                                        if len > 0 {
                                            let _ = payload
                                                .extend_from_slice(&name_bytes[..len as usize]);
                                        }
                                    }
                                    for i in 0..3 {
                                        let address = if let Some(ref bond) = state.bonds[i] {
                                            crate::ble::format_default_peer_name(
                                                bond.identity.addr.addr.raw(),
                                            )
                                        } else {
                                            heapless::String::new()
                                        };
                                        let address_bytes = address.as_bytes();
                                        let len = address_bytes.len().min(64) as u8;
                                        let _ = payload.push(len);
                                        if len > 0 {
                                            let _ = payload
                                                .extend_from_slice(&address_bytes[..len as usize]);
                                        }
                                    }
                                    let base = state.device_name.as_bytes();
                                    let _ = payload.push(base.len() as u8);
                                    let _ = payload.extend_from_slice(base);
                                });
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    &payload,
                                )
                                .await;
                            }
                            [USB_COMMAND_ACTIVATE_SLOT, slot @ 0..=2] => {
                                let bluetooth_enabled = ble::KEYBOARD_STATE
                                    .lock(|state| state.borrow().bluetooth_enabled);
                                let result = if bluetooth_enabled
                                    && run_ble_control(ble::BleCommand::ActivateSlot(
                                        *slot as usize,
                                    ))
                                    .await
                                {
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
                            [USB_COMMAND_CANCEL_PAIRING] => {
                                let result =
                                    if run_ble_control(ble::BleCommand::CancelPairing).await {
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
                            [USB_COMMAND_SET_BLUETOOTH_ENABLED, enabled @ 0..=1] => {
                                let result = if run_ble_control(
                                    ble::BleCommand::SetBluetoothEnabled(*enabled != 0),
                                )
                                .await
                                {
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
                            [USB_COMMAND_CLEAR_SLOT, slot @ 0..=2] => {
                                let slot = *slot as usize;
                                let result =
                                    if run_ble_control(ble::BleCommand::ClearSlot(slot)).await {
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
                            [USB_COMMAND_SET_DEVICE_NAME, name @ ..] => {
                                let ok = match core::str::from_utf8(name) {
                                    Ok(s) => {
                                        let mut heap_str = heapless::String::<24>::new();
                                        !s.trim().is_empty()
                                            && heap_str.push_str(s.trim()).is_ok()
                                            && run_ble_control(ble::BleCommand::SetDeviceName(
                                                heap_str,
                                            ))
                                            .await
                                    }
                                    Err(_) => false,
                                };
                                let kind = if ok {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                };
                                let body = if ok {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BAD_REQUEST][..]
                                };
                                webusb_reply(&mut transport, kind, header.request_id, body).await;
                            }
                            [USB_COMMAND_TYPE_TEXT, text @ ..] => {
                                let hid_ready = ble::KEYBOARD_STATE.lock(|state| {
                                    let state = state.borrow();
                                    state.link_state == ble::BleLinkState::Connected
                                        && state.hid_ready
                                });
                                let error = if !hid_ready {
                                    protocol::spec::ERROR_HID_NOT_READY
                                } else {
                                    match core::str::from_utf8(text) {
                                        Ok(s)
                                            if s.is_ascii()
                                                && s.chars()
                                                    .all(|ch| ble::ascii_to_hid(ch).is_some()) =>
                                        {
                                            let mut heap_str = heapless::String::<256>::new();
                                            if heap_str.push_str(s).is_err() {
                                                USB_ERROR_BAD_REQUEST
                                            } else {
                                                run_ble_control_result(ble::BleCommand::TypeString(
                                                    heap_str,
                                                ))
                                                .await
                                            }
                                        }
                                        _ => protocol::spec::ERROR_UNSUPPORTED_CHARACTER,
                                    }
                                };
                                let result = if error == 0 {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                };
                                let body = &[error][..];
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_SET_SLOT_NAME, slot, name @ ..] if *slot <= 2 => {
                                let slot = *slot as usize;
                                let ok = match core::str::from_utf8(name) {
                                    Ok(s) => {
                                        let mut heap_str = heapless::String::<32>::new();
                                        !s.trim().is_empty()
                                            && heap_str.push_str(s.trim()).is_ok()
                                            && run_ble_control(ble::BleCommand::SetSlotName(
                                                slot, heap_str,
                                            ))
                                            .await
                                    }
                                    Err(_) => false,
                                };
                                let result = if ok {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                };
                                let body = if ok {
                                    &[0][..]
                                } else {
                                    &[USB_ERROR_BAD_REQUEST][..]
                                };
                                webusb_reply(&mut transport, result, header.request_id, body).await;
                            }
                            [USB_COMMAND_FACTORY_RESET] => {
                                let ok = run_ble_control(ble::BleCommand::FactoryReset).await;
                                let kind = if ok {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                };
                                let body = if ok { &[0][..] } else { &[USB_ERROR_BUSY][..] };
                                webusb_reply(&mut transport, kind, header.request_id, body).await;
                            }
                            [USB_COMMAND_REBOOT_BOOTLOADER, ..] => {
                                crate::log_msg!("WEBUSB:REBOOT_TO_BOOTLOADER");
                                let sequence = ble::request_persist();
                                let persisted = wait_persist(sequence).await;
                                if !persisted {
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[USB_ERROR_BUSY],
                                    )
                                    .await;
                                } else {
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Response,
                                        header.request_id,
                                        b"BOOTLOADER",
                                    )
                                    .await;
                                    Timer::after(Duration::from_millis(200)).await;
                                    usb_detach::reset_after_usb_detach().await;
                                }
                            }
                            [USB_COMMAND_GET_LOGS] => {
                                let mut buf = heapless::Vec::<u8, 512>::new();
                                crate::with_logs(|line| {
                                    if buf.len() + line.len() < 512 {
                                        let _ = buf.extend_from_slice(line.as_bytes());
                                        let _ = buf.push(b'\n');
                                    }
                                });
                                webusb_reply(
                                    &mut transport,
                                    protocol::UsbFrameKind::Response,
                                    header.request_id,
                                    &buf,
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
