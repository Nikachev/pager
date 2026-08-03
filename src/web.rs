use defmt::warn;
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::pipe::Pipe;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;

pub static OTA_PIPE: Pipe<ThreadModeRawMutex, 16384> = Pipe::new();
pub static OTA_COMMAND_SIGNAL: Signal<ThreadModeRawMutex, OtaCommand> = Signal::new();
pub static OTA_CANCEL_SIGNAL: Signal<ThreadModeRawMutex, ()> = Signal::new();
pub static OTA_READY_SIGNAL: Signal<ThreadModeRawMutex, Result<(), ()>> = Signal::new();
pub static OTA_RESULT_SIGNAL: Signal<ThreadModeRawMutex, Result<(), ()>> = Signal::new();

#[derive(Clone, Copy)]
pub enum OtaCommand {
    Start {
        content_len: usize,
        expected_crc: u32,
        target_start: u32,
        target_slot: u32,
    },
    Cancel,
}

#[embassy_executor::task]
pub async fn ota_consumer_task(
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>,
) -> ! {
    loop {
        let cmd = OTA_COMMAND_SIGNAL.wait().await;
        let (content_len, expected_crc, target_start, target_slot) = match cmd {
            OtaCommand::Start {
                content_len,
                expected_crc,
                target_start,
                target_slot,
            } => (content_len, expected_crc, target_start, target_slot),
            OtaCommand::Cancel => {
                OTA_PIPE.clear();
                continue;
            }
        };

        crate::log_msg!("OTA Consumer started for {} bytes", content_len);
        OTA_PIPE.clear();
        OTA_CANCEL_SIGNAL.reset();

        let mut flash_guard = None;
        for _ in 0..50 {
            if let Ok(guard) = flash_mutex.try_lock() {
                flash_guard = Some(guard);
                break;
            }
            Timer::after(Duration::from_millis(100)).await;
        }

        let mut flash = match flash_guard {
            Some(g) => g,
            None => {
                crate::log_msg!("OTA Consumer error: flash_mutex locked");
                OTA_READY_SIGNAL.signal(Err(()));
                OTA_PIPE.clear();
                continue;
            }
        };

        crate::log_msg!(
            "OTA Consumer ready for streaming {} bytes (on-the-fly page erasing)",
            content_len
        );
        // The producer must not write before this point: this task clears the
        // pipe above, so accepting body bytes earlier can silently discard the
        // first part of an update and leave both tasks waiting forever.
        OTA_READY_SIGNAL.signal(Ok(()));

        let mut writer = crate::flash::OtaWriter::new(
            &mut *flash,
            target_start,
            target_start + MAX_BIN_SIZE as u32,
        );
        let mut total_written = 0;
        let mut crc = crate::flash::CRC32_INIT;
        let mut write_err = false;
        let mut read_buf = [0u8; 1024];

        while total_written < content_len && !write_err {
            let to_read = core::cmp::min(read_buf.len(), content_len - total_written);
            let mut pipe = &OTA_PIPE;
            let read_fut = embedded_io_async::Read::read(&mut pipe, &mut read_buf[..to_read]);
            let cancel_fut = OTA_CANCEL_SIGNAL.wait();

            let n = match embassy_futures::select::select(read_fut, cancel_fut).await {
                embassy_futures::select::Either::First(Ok(n)) if n > 0 => n,
                embassy_futures::select::Either::First(_) => {
                    write_err = true;
                    break;
                }
                embassy_futures::select::Either::Second(_) => {
                    crate::log_msg!("OTA Consumer: received cancellation signal");
                    write_err = true;
                    break;
                }
            };

            if let Err(e) = writer.write_chunk(&read_buf[..n]).await {
                crate::log_msg!("OTA Consumer write error: {:?}", e);
                write_err = true;
                break;
            }
            crc = crate::flash::crc32_update(crc, &read_buf[..n]);
            total_written += n;
        }

        let mut ready_to_validate = false;
        if !write_err
            && total_written == content_len
            && crate::flash::crc32_finalize(crc) == expected_crc
        {
            if let Err(e) = writer.flush().await {
                crate::log_msg!("OTA Consumer flush error: {:?}", e);
                OTA_RESULT_SIGNAL.signal(Err(()));
            } else {
                ready_to_validate = true;
            }
        } else {
            crate::log_msg!(
                "OTA Consumer write rejected: {} / {}, checksum={:08x}",
                total_written,
                content_len,
                crate::flash::crc32_finalize(crc)
            );
            OTA_RESULT_SIGNAL.signal(Err(()));
        }

        // A cancelled, corrupt, or incomplete transfer must never leave a
        // bootable-looking manifest in the inactive slot. The writer is no
        // longer used, so its mutable flash borrow ends before validation.
        let mut completed = false;
        if ready_to_validate {
            match crate::flash::package_targets_slot(
                &mut *flash,
                target_start,
                content_len,
                target_slot,
            )
            .await
            {
                Ok(true) => {
                    crate::log_msg!(
                        "OTA Consumer successfully finished writing {} bytes",
                        total_written
                    );
                    OTA_RESULT_SIGNAL.signal(Ok(()));
                    completed = true;
                }
                Ok(false) => {
                    crate::log_msg!("OTA package target slot mismatch");
                    OTA_RESULT_SIGNAL.signal(Err(()));
                }
                Err(error) => {
                    crate::log_msg!("OTA manifest read error: {:?}", error);
                    OTA_RESULT_SIGNAL.signal(Err(()));
                }
            }
        }
        if !completed {
            if let Err(error) =
                crate::flash::invalidate_staging_manifest(&mut *flash, target_start).await
            {
                crate::log_msg!("OTA Consumer could not invalidate manifest: {:?}", error);
            }
        }

        crate::flash::stop_flashing();
        OTA_PIPE.clear();
    }
}

/// Entire A/B slot package, including its signed 4 KiB manifest page.
pub const MAX_BIN_SIZE: usize = 488 * 1024;
pub const SLOT_A_MANIFEST_ADDR: u32 = 0x0000_8000;
pub const SLOT_B_MANIFEST_ADDR: u32 = 0x0008_2000;

pub fn running_slot() -> u32 {
    if option_env!("PAGER_SLOT") == Some("B") {
        1
    } else {
        0
    }
}

pub fn inactive_slot() -> u32 {
    1 - running_slot()
}

pub fn inactive_slot_manifest_addr() -> u32 {
    if inactive_slot() == 0 {
        SLOT_A_MANIFEST_ADDR
    } else {
        SLOT_B_MANIFEST_ADDR
    }
}

const HTTP_200_SUCCESS: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
const HTTP_400_MISSING_SLOT: &str =
    "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nMissing slot";

const USB_USBPULLUP: *mut u32 = 0x40027504 as *mut u32;
const USB_ENABLE: *mut u32 = 0x40027500 as *mut u32;

/// Detach USB long enough for macOS to discard its CDC-NCM state before the
/// bootloader and then the new application enumerate again. Both OTA paths
/// use this routine so serial DFU cannot leave a stale NCM interface behind.
pub async fn reset_after_usb_detach() -> ! {
    unsafe {
        core::ptr::write_volatile(USB_USBPULLUP, 0);
        core::ptr::write_volatile(USB_ENABLE, 0);
    }
    Timer::after(Duration::from_millis(3000)).await;
    crate::flash::reset_to_bootloader()
}

// Web server task serving the responsive HTML page on port 80 and handling requests
//
// Stability notes (reworked from the original):
//  * The TCP socket pool in the embassy_net Stack is sized with headroom
//    (see StackResources<8> in main.rs) so a stalled connection can never
//    starve the server or the DHCP server.
//  * Every accepted connection gets a bounded 8s I/O timeout, so a dead NCM
//    link (or a client that vanishes mid-request) releases its socket instead
//    of hanging the single accept loop forever.
//  * Each request is fully handled and the socket closed before the next
//    accept, keeping memory use flat and the parser state clean.
#[embassy_executor::task]
pub async fn web_task(stack: Stack<'static>) -> ! {
    static RX_BUFFER: static_cell::StaticCell<[u8; 8192]> = static_cell::StaticCell::new();
    static TX_BUFFER: static_cell::StaticCell<[u8; 4096]> = static_cell::StaticCell::new();
    let rx_buffer = RX_BUFFER.init([0u8; 8192]);
    let tx_buffer = TX_BUFFER.init([0u8; 4096]);
    let mut buf = [0u8; 2048]; // Buffered HTTP headers

    loop {
        buf.fill(0);
        let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(8)));

        crate::log_msg!("Web server listening on port 80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        crate::log_msg!("Connection accepted from {:?}", socket.remote_endpoint());

        if crate::flash::IS_FLASHING.load(core::sync::atomic::Ordering::SeqCst) {
            crate::log_msg!("Web server: DFU in progress, returning 503");
            let response = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 22\r\nConnection: close\r\n\r\nDFU Update In Progress";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            continue;
        }

        // Read initial data to locate end of HTTP headers
        let mut read_len = 0;
        loop {
            match socket.read(&mut buf[read_len..]).await {
                Ok(0) => break,
                Ok(n) => {
                    read_len += n;
                    if find_subsequence(&buf[..read_len], b"\r\n\r\n").is_some() {
                        break;
                    }
                    if read_len >= buf.len() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("read error: {:?}", e);
                    break;
                }
            }
        }

        if read_len == 0 {
            socket.close();
            continue;
        }

        let headers_end = match find_subsequence(&buf[..read_len], b"\r\n\r\n") {
            Some(idx) => idx,
            None => {
                let response =
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }
        };

        let request_line = &buf[..headers_end];

        if request_matches(request_line, "POST", "/update") {
            // Web OTA upload handler
            let content_len = match parse_content_length(request_line) {
                Some(len) => len,
                None => {
                    let response = "HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                    socket.close();
                    continue;
                }
            };

            let expected_crc = match parse_hex_u32_header(request_line, b"x-pager-crc32:") {
                Some(crc) => crc,
                None => {
                    let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 28\r\nConnection: close\r\n\r\nMissing X-Pager-CRC32 header";
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                    socket.close();
                    continue;
                }
            };

            if !crate::flash::try_start_flashing() {
                crate::log_msg!("Web OTA: DFU already in progress, returning 503");
                let response = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 22\r\nConnection: close\r\n\r\nDFU Update In Progress";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            if !(crate::flash::MANIFEST_PAGE_SIZE < content_len && content_len <= MAX_BIN_SIZE) {
                warn!("Upload size exceeds limit");
                crate::flash::stop_flashing();
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 20\r\nConnection: close\r\n\r\nInvalid package size";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            if header_value(request_line, "expect")
                .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
            {
                let continue_resp = "HTTP/1.1 100 Continue\r\n\r\n";
                let _ = socket.write_all(continue_resp.as_bytes()).await;
            }

            let body_start = headers_end + 4;
            let initial_body_len = read_len - body_start;

            crate::log_msg!(
                "OTA update request received. Size: {} bytes, initial_body: {}",
                content_len,
                initial_body_len
            );

            // Prepare signals and notify Consumer task
            OTA_PIPE.clear();
            OTA_READY_SIGNAL.reset();
            OTA_RESULT_SIGNAL.reset();
            OTA_COMMAND_SIGNAL.signal(OtaCommand::Start {
                content_len,
                expected_crc,
                target_start: inactive_slot_manifest_addr(),
                target_slot: inactive_slot(),
            });

            // Wait until the consumer has cleared the pipe and acquired flash
            // access. Without this handshake, its setup clear can race the
            // first incoming body bytes, causing the uploader to stall once
            // the TCP receive window fills.
            if OTA_READY_SIGNAL.wait().await.is_err() {
                crate::log_msg!("OTA Consumer could not acquire flash access");
                crate::flash::stop_flashing();
                let response = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 22\r\nConnection: close\r\n\r\nFlash temporarily busy";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            let mut total_read = 0;
            let mut read_error = false;
            let mut consumer_result = None;
            let mut crc = crate::flash::CRC32_INIT;

            // Push initial body block if any to Pipe
            if initial_body_len > 0 {
                let bytes_to_process = core::cmp::min(initial_body_len, content_len);
                let mut pipe = &OTA_PIPE;
                let write_fut = embedded_io_async::Write::write_all(
                    &mut pipe,
                    &buf[body_start..body_start + bytes_to_process],
                );
                match embassy_futures::select::select(write_fut, OTA_RESULT_SIGNAL.wait()).await {
                    embassy_futures::select::Either::First(Ok(())) => {
                        crc = crate::flash::crc32_update(
                            crc,
                            &buf[body_start..body_start + bytes_to_process],
                        );
                        total_read += bytes_to_process;
                    }
                    embassy_futures::select::Either::First(Err(_)) => read_error = true,
                    embassy_futures::select::Either::Second(result) => {
                        consumer_result = Some(result)
                    }
                }
            }

            // Stream remaining body directly into RAM Pipe (microsecond speed!)
            let mut read_buf = [0u8; 1024];
            while !read_error && total_read < content_len {
                let to_read = core::cmp::min(read_buf.len(), content_len - total_read);
                match socket.read(&mut read_buf[..to_read]).await {
                    Ok(0) => {
                        crate::log_msg!("Socket closed prematurely by client");
                        read_error = true;
                        break;
                    }
                    Ok(n) => {
                        let mut pipe = &OTA_PIPE;
                        let write_fut =
                            embedded_io_async::Write::write_all(&mut pipe, &read_buf[..n]);
                        match embassy_futures::select::select(write_fut, OTA_RESULT_SIGNAL.wait())
                            .await
                        {
                            embassy_futures::select::Either::First(Ok(())) => {
                                crc = crate::flash::crc32_update(crc, &read_buf[..n]);
                                total_read += n;
                            }
                            embassy_futures::select::Either::First(Err(_)) => {
                                read_error = true;
                                break;
                            }
                            embassy_futures::select::Either::Second(result) => {
                                consumer_result = Some(result);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        crate::log_msg!("Socket read error: {:?}", e);
                        read_error = true;
                        break;
                    }
                }
            }

            if !read_error
                && total_read == content_len
                && crate::flash::crc32_finalize(crc) != expected_crc
            {
                crate::log_msg!("OTA checksum mismatch");
                OTA_CANCEL_SIGNAL.signal(());
                OTA_PIPE.clear();
                crate::flash::stop_flashing();
                let response = "HTTP/1.1 422 Unprocessable Content\r\nContent-Length: 17\r\nConnection: close\r\n\r\nChecksum mismatch";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            if let Some(Err(())) = consumer_result {
                crate::log_msg!("OTA Consumer failed while receiving upload");
                crate::flash::stop_flashing();
                let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            if read_error || total_read < content_len {
                crate::log_msg!(
                    "OTA Producer read incomplete ({} / {}), cancelling Consumer",
                    total_read,
                    content_len
                );
                OTA_COMMAND_SIGNAL.signal(OtaCommand::Cancel);
                OTA_CANCEL_SIGNAL.signal(());
                OTA_PIPE.clear();
                crate::flash::stop_flashing();
                let response =
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            // Wait for Consumer to finish writing to Flash
            crate::log_msg!(
                "OTA Producer finished reading socket. Awaiting Consumer flash completion..."
            );
            let result = match consumer_result {
                Some(result) => result,
                None => OTA_RESULT_SIGNAL.wait().await,
            };

            if result.is_ok() {
                crate::log_msg!("Staging complete! Sending success HTTP response...");
                let response = HTTP_200_SUCCESS;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();

                // Wait 500ms for macOS network stack to settle after TCP close
                Timer::after(Duration::from_millis(500)).await;

                crate::log_msg!("Signed package staged; resetting into bootloader");
                crate::flash::stop_flashing();
                reset_after_usb_detach().await;
            } else {
                crate::log_msg!("OTA Consumer failed to write to flash");
                crate::flash::stop_flashing();
                let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }
        } else if request_matches(request_line, "GET", "/keyboard/state") {
            let (active_slot, bonds, pairing_mode) = crate::ble::KEYBOARD_STATE.lock(|state| {
                let s = state.borrow();
                (s.active_slot, s.bonds.clone(), s.pairing_mode)
            });
            let mut json_body = heapless::String::<256>::new();
            let _ = core::fmt::write(
                &mut json_body,
                format_args!(
                    "{{\"slots\":[{{\"id\":0,\"active\":{},\"bonded\":{}}},{{\"id\":1,\"active\":{},\"bonded\":{}}},{{\"id\":2,\"active\":{},\"bonded\":{}}}],\"pairing_mode\":{}}}",
                    active_slot == 0, bonds[0].is_some(),
                    active_slot == 1, bonds[1].is_some(),
                    active_slot == 2, bonds[2].is_some(),
                    pairing_mode
                ),
            );
            let mut response_header = heapless::String::<128>::new();
            let _ = core::fmt::write(
                &mut response_header,
                format_args!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    json_body.len()
                ),
            );
            let _ = socket.write_all(response_header.as_bytes()).await;
            let _ = socket.write_all(json_body.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "POST", "/keyboard/switch") {
            let slot_idx = parse_slot(request_line);

            if let Some(slot) = slot_idx {
                crate::ble::KEYBOARD_STATE.lock(|state| {
                    let mut s = state.borrow_mut();
                    s.active_slot = slot;
                    s.pairing_mode = false;
                });
                // Flash writes run in the dedicated persistence task. Keeping
                // them out of this single HTTP task prevents a long NVMC/MPSL
                // operation from making every endpoint unresponsive.
                crate::ble::PERSIST_STATE.signal(());
                let response = if crate::ble::try_send_command(
                    crate::ble::BleCommand::RestartAdvertising,
                ) {
                    HTTP_200_SUCCESS
                } else {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = HTTP_400_MISSING_SLOT;
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "POST", "/keyboard/pair") {
            crate::ble::KEYBOARD_STATE.lock(|state| {
                let mut s = state.borrow_mut();
                // A host can forget this keyboard while the keyboard still
                // retains its old LTK.  A new pairing must replace that stale
                // key, otherwise the old connected central keeps advertising
                // disabled and the newly-forgotten host cannot discover us.
                let active_slot = s.active_slot;
                s.bonds[active_slot] = None;
                s.pairing_mode = true;
            });
            crate::ble::PERSIST_STATE.signal(());
            // Drop the current GATT connection so the BLE task can rebuild
            // its live bond database from the now-empty persistent slot
            // before advertising again. Mutating that database in this HTTP
            // task races a connected central and leaves stale LTKs active.
            let response = if crate::ble::try_send_command(
                crate::ble::BleCommand::RestartAdvertising,
            ) {
                HTTP_200_SUCCESS
            } else {
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            };
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "POST", "/keyboard/disconnect") {
            if crate::ble::try_send_command(crate::ble::BleCommand::Disconnect) {
                let _ = socket.write_all(HTTP_200_SUCCESS.as_bytes()).await;
            } else {
                let _ = socket
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "POST", "/keyboard/delete") {
            let slot_idx = parse_slot(request_line);

            if let Some(slot) = slot_idx {
                crate::ble::KEYBOARD_STATE.lock(|state| {
                    let mut s = state.borrow_mut();
                    s.bonds[slot] = None;
                });
                crate::ble::PERSIST_STATE.signal(());
                let response = if crate::ble::try_send_command(
                    crate::ble::BleCommand::RestartAdvertising,
                ) {
                    HTTP_200_SUCCESS
                } else {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = HTTP_400_MISSING_SLOT;
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "POST", "/keyboard/type") {
            let content_len = parse_content_length(request_line).unwrap_or(0);
            if content_len > 0 && content_len <= 128 {
                let body_start = headers_end + 4;
                let initial_body_len = read_len - body_start;
                let mut type_buf = [0u8; 128];
                let mut total_read = 0;

                if initial_body_len > 0 {
                    let bytes_to_process = core::cmp::min(initial_body_len, content_len);
                    type_buf[..bytes_to_process]
                        .copy_from_slice(&buf[body_start..body_start + bytes_to_process]);
                    total_read += bytes_to_process;
                }

                while total_read < content_len {
                    let to_read =
                        core::cmp::min(type_buf.len() - total_read, content_len - total_read);
                    match socket
                        .read(&mut type_buf[total_read..total_read + to_read])
                        .await
                    {
                        Ok(0) => break,
                        Ok(n) => total_read += n,
                        Err(_) => break,
                    }
                }

                let response = match core::str::from_utf8(&type_buf[..total_read])
                    .ok()
                    .and_then(|s| heapless::String::<128>::try_from(s).ok())
                {
                    Some(text) if total_read == content_len => {
                        if crate::ble::try_send_command(crate::ble::BleCommand::TypeString(text)) {
                            HTTP_200_SUCCESS
                        } else {
                            crate::log_msg!("BLE command queue full during type request");
                            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        }
                    }
                    _ => "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nInvalid text",
                };
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nInvalid size";
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "GET", "/health") {
            let mut body = heapless::String::<192>::new();
            let _ = core::fmt::write(
                &mut body,
                format_args!(
                    "{{\"flashing\":{},\"dropped_logs\":{},\"dropped_ble_commands\":{},\"firmware_version\":{},\"slot\":{},\"ota_target_slot\":{}}}",
                    crate::flash::IS_FLASHING.load(core::sync::atomic::Ordering::Relaxed),
                    crate::DROPPED_LOGS.load(core::sync::atomic::Ordering::Relaxed),
                    crate::ble::DROPPED_COMMANDS.load(core::sync::atomic::Ordering::Relaxed),
                    crate::flash::installed_version(),
                    running_slot(),
                    inactive_slot(),
                ),
            );
            let mut header = heapless::String::<128>::new();
            let _ = core::fmt::write(
                &mut header,
                format_args!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "GET", "/logs") {
            // Keep the response below the TCP send buffer. Streaming the full
            // 32-line history (up to 4 KiB) can fill it before the peer has
            // consumed the response, blocking this single-connection server.
            let logs = crate::get_logs();
            let mut body = heapless::String::<1024>::new();
            let first = logs.len().saturating_sub(8);
            for line in logs.iter().skip(first) {
                if body.push_str(line.as_str()).is_err() || body.push('\n').is_err() {
                    break;
                }
            }

            let mut headers = heapless::String::<128>::new();
            let _ = core::fmt::write(
                &mut headers,
                format_args!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
            );
            let _ = socket.write_all(headers.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
        } else if request_matches(request_line, "GET", "/")
            || request_matches(request_line, "GET", "/index.html")
            || request_matches(request_line, "GET", "/ble_client.html")
        {
            // Serve the control page and the standalone Web Bluetooth client.
            let html = if request_matches(request_line, "GET", "/ble_client.html") {
                include_str!("../ble_client.html")
            } else {
                include_str!("index.html")
            };
            let mut headers = heapless::String::<128>::new();
            let _ = core::fmt::write(
                &mut headers,
                format_args!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    html.len()
                ),
            );
            let _ = socket.write_all(headers.as_bytes()).await;
            // The page is larger than the TCP send buffer. Chunking lets the
            // network runner make forward progress between writes.
            for chunk in html.as_bytes().chunks(1024) {
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
            }
            let _ = socket.flush().await;
            socket.close();
        } else {
            // 404 Not Found
            let response =
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
        }
    }
}

// Helper functions for raw HTTP parsing in a no_std environment
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn request_target(headers: &[u8]) -> Option<(&str, &str)> {
    let request_line = core::str::from_utf8(headers).ok()?.split("\r\n").next()?;
    let mut fields = request_line.split_ascii_whitespace();
    let method = fields.next()?;
    let target = fields.next()?;
    (fields.next() == Some("HTTP/1.1") && fields.next().is_none()).then_some((method, target))
}

fn request_matches(headers: &[u8], method: &str, path: &str) -> bool {
    request_target(headers).is_some_and(|(actual_method, target)| {
        actual_method == method
            && target
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('?'))
    })
}

fn header_value<'a>(headers: &'a [u8], name: &str) -> Option<&'a str> {
    let mut value = None;
    for line in core::str::from_utf8(headers).ok()?.split("\r\n").skip(1) {
        let (header_name, header_value) = line.split_once(':')?;
        if header_name.eq_ignore_ascii_case(name) && value.replace(header_value.trim()).is_some() {
            return None;
        }
    }
    value
}

fn parse_slot(headers: &[u8]) -> Option<usize> {
    let (_, target) = request_target(headers)?;
    let query = target.split_once('?')?.1;
    let value = query.split('&').find_map(|item| {
        item.split_once('=')
            .and_then(|(name, value)| (name == "slot").then_some(value))
    })?;
    match value {
        "0" => Some(0),
        "1" => Some(1),
        "2" => Some(2),
        _ => None,
    }
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    header_value(headers, "content-length")?.parse().ok()
}

fn parse_hex_u32_header(headers: &[u8], name: &[u8]) -> Option<u32> {
    let name = core::str::from_utf8(name).ok()?.trim_end_matches(':');
    let value = header_value(headers, name)?;
    let value = value.strip_prefix("0x").unwrap_or(value);
    u32::from_str_radix(value, 16).ok()
}
