use defmt::*;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embedded_io_async::Write;
use embedded_storage_async::nor_flash::NorFlash as _;
use embedded_storage_async::nor_flash::ReadNorFlash as _;
use embassy_time::{Duration, Timer};
use crate::flash::copy_and_reset;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;

pub const MAX_BIN_SIZE: usize = 400 * 1024;
pub const STAGING_START_ADDR: u32 = 0x8C000;
pub const ACTIVE_START_ADDR: u32 = 0x27000;

// Persistent BLE bonding keys are stored in the last flash page before the
// bootloader config region. Must NOT overlap with staging (ends at 0xF0000).
pub const BONDS_STORAGE_ADDR: u32 = 0xF0000;

// A monotonically increasing boot counter, folded into the BLE address so
// every reboot presents a new device identity to the host (avoids stale-bond
// failures). Kept in its own flash page, well clear of staging/app/bootloader.
pub const BOOT_COUNT_ADDR: u32 = 0xEE000;
pub const BOOT_COUNT_PAGE: u32 = 0xEE000;

pub async fn next_boot_count(flash: &mut nrf_softdevice::Flash) -> u32 {
    let mut buf = [0u8; 4];
    let _ = flash.read(BOOT_COUNT_ADDR, &mut buf).await;
    let count = u32::from_le_bytes(buf);
    let next = count.wrapping_add(1);
    let _ = flash
        .erase(BOOT_COUNT_PAGE, BOOT_COUNT_PAGE + 4096)
        .await;
    let _ = flash.write(BOOT_COUNT_ADDR, &next.to_le_bytes()).await;
    next
}

const USB_USBPULLUP: *mut u32 = 0x40027504 as *mut u32;

// Web server task serving the responsive HTML page on port 80 and handling requests
#[embassy_executor::task]
pub async fn web_task(
    stack: Stack<'static>,
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_softdevice::Flash>,
) -> ! {
    let mut rx_buffer = [0u8; 2048];
    let mut tx_buffer = [0u8; 2048];
    let mut buf = [0u8; 2048]; // Buffered HTTP headers

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        crate::log_msg!("Web server listening on port 80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        crate::log_msg!("Connection accepted from {:?}", socket.remote_endpoint());

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
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }
        };

        let request_line = &buf[..headers_end];

        if starts_with(request_line, b"POST /update") {
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

            crate::log_msg!("OTA update request received. Size: {} bytes", content_len);

            if content_len > MAX_BIN_SIZE {
                warn!("Upload size exceeds limit");
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\nConnection: close\r\n\r\nFile exceeds 400KB limit";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            let mut flash = flash_mutex.lock().await;
            let mut writer = crate::flash::OtaWriter::new(&mut *flash, STAGING_START_ADDR);

            if let Err(e) = writer.erase(content_len).await {
                warn!("Flash erase error: {:?}", e);
            }

            let body_start = headers_end + 4;
            let initial_body_len = read_len - body_start;
            let mut total_read = 0;
            let mut write_error = false;

            // Write initial block
            if initial_body_len > 0 {
                let bytes_to_process = core::cmp::min(initial_body_len, content_len);
                if let Err(e) = writer.write_chunk(&buf[body_start..body_start + bytes_to_process]).await {
                    warn!("Staging write error: {:?}", e);
                    write_error = true;
                } else {
                    total_read += bytes_to_process;
                }
            }

            // Read remaining data from network socket
            let mut read_buf = [0u8; 1024];
            while !write_error && total_read < content_len {
                let to_read = core::cmp::min(read_buf.len(), content_len - total_read);
                match socket.read(&mut read_buf[..to_read]).await {
                    Ok(0) => {
                        warn!("Socket closed early during upload");
                        break;
                    }
                    Ok(n) => {
                        if let Err(e) = writer.write_chunk(&read_buf[..n]).await {
                            warn!("Staging write error: {:?}", e);
                            write_error = true;
                            break;
                        }
                        total_read += n;
                    }
                    Err(e) => {
                        warn!("Socket read error: {:?}", e);
                        break;
                    }
                }
            }

            if !write_error && total_read == content_len {
                if let Err(e) = writer.flush().await {
                    warn!("Flash final write error: {:?}", e);
                }

                crate::log_msg!("Staging complete! Sending success HTTP response...");
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();

                // Wait 500ms for macOS network stack to settle after TCP close
                Timer::after(Duration::from_millis(500)).await;

                // Disable USB pull-up for 3000ms to ensure host detects clean disconnect
                unsafe {
                    core::ptr::write_volatile(USB_USBPULLUP, 0);
                }
                Timer::after(Duration::from_millis(3000)).await;

                crate::log_msg!("Initiating Active Bank self-flash and system reset!");
                unsafe {
                    copy_and_reset(STAGING_START_ADDR, ACTIVE_START_ADDR, content_len as u32);
                }
            } else {
                crate::log_msg!("Upload incomplete. Expected {} but got {}", content_len, total_read);
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
            }
        } else if starts_with(request_line, b"GET /keyboard/state") {
            let mut response_body = heapless::String::<512>::new();
            let state_str = crate::ble::KEYBOARD_STATE.lock(|state| {
                let s = state.borrow();
                let mut b = heapless::String::<256>::new();
                let _ = b.push_str("{\"slots\":[");
                for i in 0..3 {
                    let active = s.active_slot == i;
                    let bonded = s.bonds[i].is_some();
                    let mut slot_str = heapless::String::<64>::new();
                    let _ = core::fmt::write(&mut slot_str, format_args!(
                        "{{\"id\":{},\"active\":{},\"bonded\":{}}}",
                        i, active, bonded
                    ));
                    let _ = b.push_str(slot_str.as_str());
                    if i < 2 {
                        let _ = b.push_str(",");
                    }
                }
                let mut end_str = heapless::String::<64>::new();
                let _ = core::fmt::write(&mut end_str, format_args!(
                    "],\"pairing_mode\":{}}}",
                    s.pairing_mode
                ));
                let _ = b.push_str(end_str.as_str());
                b
            });
            let _ = core::fmt::write(&mut response_body, format_args!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                state_str.len(),
                state_str.as_str()
            ));
            let _ = socket.write_all(response_body.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(100)).await;
        } else if starts_with(request_line, b"POST /keyboard/switch") {
            let slot_idx = if let Some(_) = find_subsequence(request_line, b"slot=0") { Some(0) }
                           else if let Some(_) = find_subsequence(request_line, b"slot=1") { Some(1) }
                           else if let Some(_) = find_subsequence(request_line, b"slot=2") { Some(2) }
                           else { None };

            if let Some(slot) = slot_idx {
                crate::ble::KEYBOARD_STATE.lock(|state| {
                    let mut s = state.borrow_mut();
                    s.active_slot = slot;
                    s.pairing_mode = false;
                });
                let _ = crate::ble::BLE_COMMANDS.try_send(crate::ble::BleCommand::RestartAdvertising);
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nMissing slot";
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(100)).await;
        } else if starts_with(request_line, b"POST /keyboard/pair") {
            crate::ble::KEYBOARD_STATE.lock(|state| {
                let mut s = state.borrow_mut();
                s.pairing_mode = true;
            });
            let _ = crate::ble::BLE_COMMANDS.try_send(crate::ble::BleCommand::Disconnect);
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(100)).await;
        } else if starts_with(request_line, b"POST /keyboard/delete") {
            let slot_idx = if let Some(_) = find_subsequence(request_line, b"slot=0") { Some(0) }
                           else if let Some(_) = find_subsequence(request_line, b"slot=1") { Some(1) }
                           else if let Some(_) = find_subsequence(request_line, b"slot=2") { Some(2) }
                           else { None };

            if let Some(slot) = slot_idx {
                let is_active = crate::ble::KEYBOARD_STATE.lock(|state| {
                    let mut s = state.borrow_mut();
                    s.bonds[slot] = None;
                    s.active_slot == slot
                });
                // Erase the bond from flash so it stays in sync with the host.
                let mut flash = flash_mutex.lock().await;
                crate::ble::erase_bond_slot(&mut flash, slot).await;
                drop(flash);
                if is_active {
                    let _ = crate::ble::BLE_COMMANDS.try_send(crate::ble::BleCommand::RestartAdvertising);
                }
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nMissing slot";
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(100)).await;
        } else if starts_with(request_line, b"POST /keyboard/type") {
            let content_len = parse_content_length(request_line).unwrap_or(0);
            if content_len > 0 && content_len <= 128 {
                let body_start = headers_end + 4;
                let initial_body_len = read_len - body_start;
                let mut type_buf = [0u8; 128];
                let mut total_read = 0;

                if initial_body_len > 0 {
                    let bytes_to_process = core::cmp::min(initial_body_len, content_len);
                    type_buf[..bytes_to_process].copy_from_slice(&buf[body_start..body_start + bytes_to_process]);
                    total_read += bytes_to_process;
                }

                while total_read < content_len {
                    let to_read = core::cmp::min(type_buf.len() - total_read, content_len - total_read);
                    match socket.read(&mut type_buf[total_read..total_read + to_read]).await {
                        Ok(0) => break,
                        Ok(n) => total_read += n,
                        Err(_) => break,
                    }
                }

                if total_read == content_len {
                    if let Ok(s) = core::str::from_utf8(&type_buf[..total_read]) {
                        if let Ok(heap_str) = heapless::String::<128>::try_from(s) {
                            let _ = crate::ble::BLE_COMMANDS.try_send(crate::ble::BleCommand::TypeString(heap_str));
                        }
                    }
                }
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
            } else {
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 12\r\nConnection: close\r\n\r\nInvalid size";
                let _ = socket.write_all(response.as_bytes()).await;
            }
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(100)).await;
        } else if starts_with(request_line, b"POST /bootloader") {
            crate::log_msg!("Rebooting to Bootloader (UF2) via HTTP...");
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();

            // Wait a bit for response to send
            Timer::after(Duration::from_millis(500)).await;

            unsafe {
                let _ = nrf_softdevice::raw::sd_power_gpregret_set(0, 0x57);
                let aircr = 0xE000ED0C as *mut u32;
                core::ptr::write_volatile(aircr, 0x05FA0004);
            }
        } else if starts_with(request_line, b"GET /logs") {
            let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(headers.as_bytes()).await;

            let logs = crate::get_logs();
            for line in logs.iter() {
                let mut formatted = heapless::String::<128>::new();
                let _ = core::fmt::write(&mut formatted, format_args!("{}\n", line));
                if let Err(e) = socket.write_all(formatted.as_bytes()).await {
                    warn!("write error: {:?}", e);
                    break;
                }
            }
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(500)).await;
        } else if starts_with(request_line, b"GET / ") || starts_with(request_line, b"GET /index.html") {
            // Serve standard web interface page
            let html = include_str!("index.html");
            let headers = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/html; charset=utf-8\r\n",
                "Connection: close\r\n",
                "\r\n"
            );

            let _ = socket.write_all(headers.as_bytes()).await;
            let _ = socket.write_all(html.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(500)).await;
        } else {
            // 404 Not Found
            let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            Timer::after(Duration::from_millis(500)).await;
        }
    }
}

// Helper functions for raw HTTP parsing in a no_std environment
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn starts_with(data: &[u8], prefix: &[u8]) -> bool {
    data.len() >= prefix.len() && &data[..prefix.len()] == prefix
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let target = b"content-length:";
    let mut found_idx = None;

    for i in 0..headers.len() {
        if i + 15 <= headers.len() {
            let chunk = &headers[i..i+15];
            let mut matched = true;
            for j in 0..15 {
                let mut c1 = chunk[j];
                if c1 >= b'A' && c1 <= b'Z' {
                    c1 = c1 - b'A' + b'a';
                }
                if c1 != target[j] {
                    matched = false;
                    break;
                }
            }
            if matched {
                found_idx = Some(i + 15);
                break;
            }
        }
    }

    let start = found_idx?;

    let mut len = 0;
    let mut found_digit = false;
    for &c in &headers[start..] {
        if c >= b'0' && c <= b'9' {
            len = len * 10 + (c - b'0') as usize;
            found_digit = true;
        } else if c == b'\r' || c == b'\n' {
            if found_digit {
                return Some(len);
            }
        } else if c == b' ' || c == b':' {
            continue;
        } else {
            if found_digit {
                return Some(len);
            }
            return None;
        }
    }

    if found_digit { Some(len) } else { None }
}

