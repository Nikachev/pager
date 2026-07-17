use defmt::*;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embedded_io_async::Write;
use embassy_time::{Duration, Timer};
use crate::flash::{raw_flash_erase, raw_flash_write_block, copy_and_reset};

pub const MAX_BIN_SIZE: usize = 480 * 1024;
pub const STAGING_START_ADDR: u32 = 0x7A000;
pub const ACTIVE_START_ADDR: u32 = 0x1000;

// Web server task serving the responsive HTML page on port 80 and handling POST /update
#[embassy_executor::task]
pub async fn web_task(stack: Stack<'static>) -> ! {
    let mut rx_buffer = [0u8; 2048];
    let mut tx_buffer = [0u8; 2048];
    let mut buf = [0u8; 2048]; // Buffered HTTP headers

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("Web server listening on port 80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Connection accepted from {:?}", socket.remote_endpoint());

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

            info!("OTA update request received. Size: {} bytes", content_len);

            if content_len > MAX_BIN_SIZE {
                warn!("Upload size exceeds limit");
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\nConnection: close\r\n\r\nFile exceeds 480KB limit";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            let page_size = 4096;
            let erase_size = (content_len + page_size - 1) & !(page_size - 1);

            info!("Erasing staging partition: {} bytes...", erase_size);
            unsafe {
                raw_flash_erase(STAGING_START_ADDR, erase_size as u32);
            }

            // Stream and write raw POST body directly to update partition.
            // Accumulate blocks in a local buffer to ensure 4-byte NVMC write alignment.
            let mut write_buffer = [0u8; 256];
            let mut write_buf_len = 0;
            let mut flash_offset = STAGING_START_ADDR;

            let body_start = headers_end + 4;
            let initial_body_len = read_len - body_start;
            let mut total_read = 0;

            let mut process_bytes = |data: &[u8]| -> Result<(), ()> {
                let mut data_idx = 0;
                while data_idx < data.len() {
                    let chunk_size = core::cmp::min(data.len() - data_idx, write_buffer.len() - write_buf_len);
                    write_buffer[write_buf_len..write_buf_len + chunk_size]
                        .copy_from_slice(&data[data_idx..data_idx + chunk_size]);
                    write_buf_len += chunk_size;
                    data_idx += chunk_size;

                    if write_buf_len == write_buffer.len() {
                        unsafe {
                            raw_flash_write_block(flash_offset, &write_buffer);
                        }
                        flash_offset += write_buffer.len() as u32;
                        write_buf_len = 0;
                    }
                }
                Ok(())
            };

            // Write initial block
            if initial_body_len > 0 {
                let bytes_to_process = core::cmp::min(initial_body_len, content_len);
                if let Err(_) = process_bytes(&buf[body_start..body_start + bytes_to_process]) {
                    warn!("Staging write error");
                    total_read = 0;
                } else {
                    total_read += bytes_to_process;
                }
            }

            // Read remaining data from network socket
            let mut read_buf = [0u8; 1024];
            while total_read < content_len {
                let to_read = core::cmp::min(read_buf.len(), content_len - total_read);
                match socket.read(&mut read_buf[..to_read]).await {
                    Ok(0) => {
                        warn!("Socket closed early during upload");
                        break;
                    }
                    Ok(n) => {
                        if let Err(_) = process_bytes(&read_buf[..n]) {
                            warn!("Staging write error");
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

            if total_read == content_len {
                // Write any remaining buffered bytes
                if write_buf_len > 0 {
                    unsafe {
                        raw_flash_write_block(flash_offset, &write_buffer[..write_buf_len]);
                    }
                }

                info!("Staging complete! Sending success HTTP response...");
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 7\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();

                // Wait 500ms for macOS network stack to settle after TCP close
                Timer::after(Duration::from_millis(500)).await;

                // Disable USB pull-up for 3000ms to ensure host detects clean disconnect
                unsafe {
                    let usbpullup = 0x40027504 as *mut u32;
                    core::ptr::write_volatile(usbpullup, 0);
                }
                Timer::after(Duration::from_millis(3000)).await;

                info!("Initiating Active Bank self-flash and system reset!");
                unsafe {
                    copy_and_reset(STAGING_START_ADDR, ACTIVE_START_ADDR, content_len as u32);
                }
            } else {
                warn!("Upload incomplete. Expected {} but got {}", content_len, total_read);
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
            }
        } else {
            // Serve standard web interface page
            let html = include_str!("index.html");
            let headers = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/html; charset=utf-8\r\n",
                "Connection: close\r\n",
                "\r\n"
            );

            if let Err(e) = socket.write_all(headers.as_bytes()).await {
                warn!("write error: {:?}", e);
            }
            if let Err(e) = socket.write_all(html.as_bytes()).await {
                warn!("write error: {:?}", e);
            }
            if let Err(e) = socket.flush().await {
                warn!("flush error: {:?}", e);
            }
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
