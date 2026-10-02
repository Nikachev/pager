//! WebUSB control plane task and frame handlers.

use crate::protocol;
use embassy_futures::select::{select, Either, Either4};
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
    let Some(mut receiver) = ble::BLE_CONTROL_RESULT.receiver() else {
        return USB_ERROR_BUSY;
    };
    let id = ble::next_control_id();
    let deadline = embassy_time::Instant::now() + Duration::from_secs(10);
    if !ble::try_send_command(ble::BleRequest {
        id,
        deadline_ticks: deadline.as_ticks(),
        command,
    }) {
        return protocol::spec::ERROR_QUEUE_FULL;
    }
    match select(
        receiver.get_and(|(completed, _)| *completed == id),
        Timer::at(deadline),
    )
    .await
    {
        Either::First((_, 0)) => 0,
        Either::First((_, 2)) => protocol::spec::ERROR_UNSUPPORTED_CHARACTER,
        Either::First((_, 3)) => protocol::spec::ERROR_CONNECTION_LOST,
        Either::First(_) | Either::Second(_) => USB_ERROR_BUSY,
    }
}

async fn run_ble_control(command: ble::BleCommand) -> bool {
    run_ble_control_result(command).await == 0
}

async fn wait_persist(sequence: u32) -> bool {
    ble::wait_persist(sequence).await
}

pub async fn webusb_reply(
    transport: &mut webusb::Transport<'static, NrfUsbDriver>,
    kind: protocol::UsbFrameKind,
    request_id: u32,
    payload: &[u8],
) {
    let mut frame = [0u8; protocol::USB_FRAME_HEADER_LEN + protocol::USB_MAX_PAYLOAD];
    if let Ok(n) = protocol::encode_usb_frame(&mut frame, kind, request_id, payload) {
        // Waiting to submit a bulk IN frame.
        crate::trace_usb_phase(3);
        // A host may leave bulk IN unpolled while using BLE. Backpressure
        // before the driver's synchronous DMA is a legitimate idle wait.
        let _ =
            crate::idle_progress(crate::HEARTBEAT_WEBUSB, transport.write_frame(&frame[..n])).await;
        crate::trace_usb_phase(4); // Bulk IN frame submitted.
    }
}

#[embassy_executor::task]
pub async fn webusb_task(mut transport: webusb::Transport<'static, NrfUsbDriver>) -> ! {
    let mut frames = protocol::FrameStream::new();
    let mut frame = protocol::OwnedFrame::new();
    let mut transfer = [0u8; 64];
    let mut pending_type: Option<(u32, u32, embassy_time::Instant)> = None;
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
        crate::idle_progress(crate::HEARTBEAT_WEBUSB, transport.wait_connection()).await;
        frames.reset();
        if let Some((_, id, _)) = pending_type.take() {
            ble::finish_type_control(id);
        }
        crate::log_msg!("WEBUSB:CONNECTED");

        loop {
            crate::signal_heartbeat(crate::HEARTBEAT_WEBUSB);
            crate::trace_usb_phase(1); // Waiting for input, events, or type completion.
            let type_deadline = pending_type
                .as_ref()
                .map(|(_, _, deadline)| *deadline)
                .unwrap_or_else(|| embassy_time::Instant::now() + Duration::from_secs(3600));
            let n = match embassy_futures::select::select4(
                crate::idle_progress(
                    crate::HEARTBEAT_WEBUSB,
                    transport.read_transfer(&mut transfer),
                ),
                events.next_message(),
                ble::TYPE_CONTROL_RESULT.wait(),
                Timer::at(type_deadline),
            )
            .await
            {
                Either4::First(Ok(0)) => continue,
                Either4::First(Ok(n)) => {
                    crate::trace_usb_receive(n);
                    crate::trace_usb_phase(2); // Parsing received bytes.
                    n
                }
                Either4::First(Err(_)) => break,
                Either4::Second(event) => {
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
                Either4::Third((id, result)) => {
                    if let Some((request_id, expected, _)) = pending_type {
                        if id == expected {
                            pending_type = None;
                            ble::finish_type_control(id);
                            let error = match result {
                                0 => 0,
                                2 => protocol::spec::ERROR_UNSUPPORTED_CHARACTER,
                                3 => protocol::spec::ERROR_CONNECTION_LOST,
                                _ => USB_ERROR_BUSY,
                            };
                            webusb_reply(
                                &mut transport,
                                if error == 0 {
                                    protocol::UsbFrameKind::Response
                                } else {
                                    protocol::UsbFrameKind::Error
                                },
                                request_id,
                                &[error],
                            )
                            .await;
                        }
                    }
                    continue;
                }
                Either4::Fourth(()) => {
                    if let Some((request_id, id, _)) = pending_type.take() {
                        ble::finish_type_control(id);
                        webusb_reply(
                            &mut transport,
                            protocol::UsbFrameKind::Error,
                            request_id,
                            &[USB_ERROR_BUSY],
                        )
                        .await;
                    }
                    continue;
                }
            };
            let mut offset = 0;
            while offset < n {
                // Fill only available capacity, then drain. A maximum frame and
                // the following header can share a packet without overflow.
                match frames.push_slice(&transfer[offset..n]) {
                    Ok(count) => offset += count,
                    Err(_) => {
                        frames.reset();
                        continue;
                    }
                }
                while let Some(result) = frames.next_frame_into(&mut frame) {
                    match result {
                        Ok(()) if frame.header.kind == protocol::UsbFrameKind::Command => {
                            let header = frame.header;
                            let payload = frame.payload();
                            crate::trace_usb_command(
                                header.request_id,
                                payload.first().copied().unwrap_or(0),
                            );
                            let injection = crate::faults::ENABLED && matches!(payload, [0xF0, _]);
                            if !injection {
                                if let Err(error) = protocol::decode_command(payload) {
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Error,
                                        header.request_id,
                                        &[match error {
                                            protocol::CommandError::BadRequest => {
                                                USB_ERROR_BAD_REQUEST
                                            }
                                            protocol::CommandError::Unsupported => {
                                                USB_ERROR_UNSUPPORTED_COMMAND
                                            }
                                        }],
                                    )
                                    .await;
                                    continue;
                                }
                            }
                            match frame.payload() {
                                [0xF0, 5] if crate::faults::ENABLED => {
                                    let ok = crate::faults::arm(5)
                                        && run_ble_control(ble::BleCommand::FactoryReset).await;
                                    webusb_reply(
                                        &mut transport,
                                        if ok {
                                            protocol::UsbFrameKind::Response
                                        } else {
                                            protocol::UsbFrameKind::Error
                                        },
                                        header.request_id,
                                        &[if ok { 0 } else { USB_ERROR_BUSY }],
                                    )
                                    .await;
                                }
                                [0xF0, action] if crate::faults::ENABLED => {
                                    let ok = crate::faults::arm(*action);
                                    webusb_reply(
                                        &mut transport,
                                        if ok {
                                            protocol::UsbFrameKind::Response
                                        } else {
                                            protocol::UsbFrameKind::Error
                                        },
                                        header.request_id,
                                        &[if ok { 0 } else { USB_ERROR_BAD_REQUEST }],
                                    )
                                    .await;
                                }
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
                                    let mut info = heapless::String::<256>::new();
                                    #[cfg(feature = "board-nice-nano-v2")]
                                    let board = "nice-nano-v2";
                                    #[cfg(feature = "board-xiao-nrf52840")]
                                    let board = "xiao-nrf52840";
                                    let _ = core::fmt::write(&mut info, format_args!(
                                    "Pager;protocol={};bootloader=single-slot;version={};board={};image_sha256=",
                                    protocol::spec::FRAME_VERSION, env!("PAGER_BUILD_VERSION"), board,
                                ));
                                    // The bootloader authenticated this running manifest. These
                                    // 32 bytes are its public image digest, within the flash map.
                                    let digest = unsafe {
                                        core::slice::from_raw_parts(
                                            (crate::layout::FIRMWARE_START + 16) as *const u8,
                                            32,
                                        )
                                    };
                                    let _ = protocol::append_hex(&mut info, digest);
                                    if crate::faults::ENABLED {
                                        let _ = info.push_str(";fault_injection=1");
                                    }
                                    webusb_reply(
                                        &mut transport,
                                        protocol::UsbFrameKind::Response,
                                        header.request_id,
                                        info.as_bytes(),
                                    )
                                    .await;
                                }
                                [USB_COMMAND_GET_STATE] => {
                                    let payload = ble::KEYBOARD_STATE
                                        .lock(|state| protocol::encode_state(&state.borrow()));
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
                                    webusb_reply(&mut transport, result, header.request_id, body)
                                        .await;
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
                                    webusb_reply(&mut transport, result, header.request_id, body)
                                        .await;
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
                                    webusb_reply(&mut transport, result, header.request_id, body)
                                        .await;
                                }
                                [USB_COMMAND_CLEAR_SLOT, slot @ 0..=2] => {
                                    let slot = *slot as usize;
                                    let result = if run_ble_control(ble::BleCommand::ClearSlot(
                                        slot,
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
                                    webusb_reply(&mut transport, result, header.request_id, body)
                                        .await;
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
                                    webusb_reply(&mut transport, kind, header.request_id, body)
                                        .await;
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
                                                    && s.chars().all(|ch| {
                                                        ble::ascii_to_hid(ch).is_some()
                                                    }) =>
                                            {
                                                let mut heap_str = heapless::String::<256>::new();
                                                if heap_str.push_str(s).is_err() {
                                                    USB_ERROR_BAD_REQUEST
                                                } else {
                                                    let id = ble::next_control_id();
                                                    let deadline = embassy_time::Instant::now()
                                                        + Duration::from_secs(10);
                                                    if pending_type.is_some()
                                                        || !ble::register_type_control(id)
                                                    {
                                                        USB_ERROR_BUSY
                                                    } else if !ble::try_send_command(
                                                        ble::BleRequest {
                                                            id,
                                                            deadline_ticks: deadline.as_ticks(),
                                                            command: ble::BleCommand::TypeString(
                                                                heap_str,
                                                            ),
                                                        },
                                                    ) {
                                                        ble::finish_type_control(id);
                                                        protocol::spec::ERROR_QUEUE_FULL
                                                    } else {
                                                        pending_type =
                                                            Some((header.request_id, id, deadline));
                                                        0
                                                    }
                                                }
                                            }
                                            _ => protocol::spec::ERROR_UNSUPPORTED_CHARACTER,
                                        }
                                    };
                                    if error != 0 {
                                        webusb_reply(
                                            &mut transport,
                                            protocol::UsbFrameKind::Error,
                                            header.request_id,
                                            &[error],
                                        )
                                        .await;
                                    }
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
                                    webusb_reply(&mut transport, result, header.request_id, body)
                                        .await;
                                }
                                [USB_COMMAND_FACTORY_RESET] => {
                                    let ok = run_ble_control(ble::BleCommand::FactoryReset).await;
                                    let kind = if ok {
                                        protocol::UsbFrameKind::Response
                                    } else {
                                        protocol::UsbFrameKind::Error
                                    };
                                    let body = if ok { &[0][..] } else { &[USB_ERROR_BUSY][..] };
                                    webusb_reply(&mut transport, kind, header.request_id, body)
                                        .await;
                                }
                                [USB_COMMAND_REBOOT_BOOTLOADER] => {
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
                                    use core::sync::atomic::Ordering::Relaxed;
                                    let stats = crate::diagnostics::Snapshot {
                                        reset: crate::RESET_REASON.load(Relaxed),
                                        logs_drop: crate::DROPPED_LOGS.load(Relaxed),
                                        history_drop: crate::HISTORY_EVICTIONS.load(Relaxed),
                                        log_high: crate::LOG_HIGH_WATER.load(Relaxed),
                                        cmd_drop: ble::DROPPED_COMMANDS.load(Relaxed),
                                        cmd_high: ble::COMMAND_HIGH_WATER.load(Relaxed),
                                        storage_error: crate::STORAGE_ERRORS.load(Relaxed),
                                        uptime_ms: embassy_time::Instant::now().as_millis(),
                                    }
                                    .format();
                                    let _ = buf.extend_from_slice(stats.as_bytes());
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
                }
            }
        }
        crate::log_msg!("WEBUSB:DISCONNECTED");
    }
}
