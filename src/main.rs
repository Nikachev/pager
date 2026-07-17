#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, peripherals, Peri};
use embassy_usb::class::cdc_ncm::embassy_net::{Device, Runner, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::{Builder, Config, UsbDevice};
use embassy_net::{Stack, StackResources};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// Bind interrupts for the USB controller and the Power controller (VBUS detection)
bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

const MTU: usize = 1514;
type MyDriver = Driver<'static, HardwareVbusDetect>;

// USB device background runner task
#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, MyDriver>) -> ! {
    device.run().await
}

// CDC-NCM network interface runner task
#[embassy_executor::task]
async fn usb_ncm_task(class: Runner<'static, MyDriver, MTU>) -> ! {
    class.run().await
}

// TCP/IP stack runner task
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, Device<'static, MTU>>) -> ! {
    runner.run().await
}

// DHCP server task
#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>) -> ! {
    use core::net::Ipv4Addr;
    use leasehund::DhcpServer;

    let mut server = DhcpServer::<32, 4>::new(
        Ipv4Addr::new(192, 168, 42, 1),   // Server IP (the board's IP)
        Ipv4Addr::new(255, 255, 255, 0), // Subnet mask
        Ipv4Addr::new(192, 168, 42, 1),   // Default gateway
        Ipv4Addr::new(8, 8, 8, 8),       // DNS server
        Ipv4Addr::new(192, 168, 42, 2),   // IP pool start address
        Ipv4Addr::new(192, 168, 42, 10),  // IP pool end address
    );

    info!("DHCP server started. Waiting for requests on port 67...");
    server.run(stack).await;
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

// RAM-Resident function that handles erasing and overwriting the Active Bank
// with the staged binary, then resets the microcontroller.
#[link_section = ".data"]
#[inline(never)]
unsafe fn copy_and_reset(src_addr: u32, dest_addr: u32, len_bytes: u32) -> ! {
    cortex_m::interrupt::disable();

    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;
    let nvmc_erasepage = 0x4001E508 as *mut u32;

    // 1. Erase destination pages (4KB per page) in Active Bank
    let page_size = 4096;
    let num_pages = (len_bytes + page_size - 1) / page_size;
    for page_idx in 0..num_pages {
        let page_addr = dest_addr + page_idx * page_size;

        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 2); // Enable Erase
        core::ptr::write_volatile(nvmc_erasepage, page_addr);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }

    // 2. Copy staged binary word by word (4 bytes / 32-bit words)
    let count_words = (len_bytes + 3) / 4;
    let src_ptr = src_addr as *const u32;
    let dest_ptr = dest_addr as *mut u32;

    for i in 0..count_words {
        let val = core::ptr::read_volatile(src_ptr.offset(i as isize));

        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 1); // Enable Write
        core::ptr::write_volatile(dest_ptr.offset(i as isize), val);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }

    // 3. System Reset via AIRCR
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
    core::ptr::write_volatile(nvmc_config, 0); // Read-Only mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}

    let aircr = 0xE000ED0C as *mut u32;
    core::ptr::write_volatile(aircr, 0x05FA0004);

    loop {}
}

// Raw Flash operations for the Web OTA Update to avoid Nvmc peripheral locking
unsafe fn raw_flash_erase(start_addr: u32, len_bytes: u32) {
    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;
    let nvmc_erasepage = 0x4001E508 as *mut u32;

    let page_size = 4096;
    let num_pages = (len_bytes + page_size - 1) / page_size;
    for page_idx in 0..num_pages {
        let page_addr = start_addr + page_idx * page_size;
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 2); // Enable Erase
        core::ptr::write_volatile(nvmc_erasepage, page_addr);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }
    core::ptr::write_volatile(nvmc_config, 0); // Return to Read mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
}

unsafe fn raw_flash_write_block(dest_addr: u32, data: &[u8]) {
    let nvmc_ready = 0x4001E400 as *mut u32;
    let nvmc_config = 0x4001E504 as *mut u32;

    let count_words = (data.len() + 3) / 4;
    let src_ptr = data.as_ptr() as *const u32;
    let dest_ptr = dest_addr as *mut u32;

    for i in 0..count_words {
        let val = core::ptr::read_volatile(src_ptr.offset(i as isize));
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
        core::ptr::write_volatile(nvmc_config, 1); // Enable Write
        core::ptr::write_volatile(dest_ptr.offset(i as isize), val);
        while core::ptr::read_volatile(nvmc_ready) == 0 {}
    }
    core::ptr::write_volatile(nvmc_config, 0); // Return to Read mode
    while core::ptr::read_volatile(nvmc_ready) == 0 {}
}

// Web server task serving the responsive HTML page on port 80 and handling POST /update
#[embassy_executor::task]
async fn web_task(stack: Stack<'static>) -> ! {
    use embassy_net::tcp::TcpSocket;
    use embedded_io_async::Write;

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

            // Limit binary size to 480KB (matches Active/Update partition bounds)
            if content_len > 480 * 1024 {
                warn!("Upload size exceeds 480KB limit");
                let response = "HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\nConnection: close\r\n\r\nFile exceeds 480KB limit";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();
                continue;
            }

            let start_addr = 0x7A000; // Staging Update Bank starts at 488KB offset
            let page_size = 4096;
            let erase_size = (content_len + page_size - 1) & !(page_size - 1);

            info!("Erasing staging partition: {} bytes...", erase_size);
            unsafe {
                raw_flash_erase(start_addr, erase_size as u32);
            }

            // Stream and write raw POST body directly to update partition.
            // Accumulate blocks in a local buffer to ensure 4-byte NVMC write alignment.
            let mut write_buffer = [0u8; 256];
            let mut write_buf_len = 0;
            let mut flash_offset = start_addr;

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
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\nSuccess";
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                socket.close();

                // Disable USB pull-up to simulate physical disconnect before flash copy
                unsafe {
                    let usbpullup = 0x40027504 as *mut u32;
                    core::ptr::write_volatile(usbpullup, 0);
                }
                // Wait for the host to register the disconnect
                Timer::after(Duration::from_millis(1000)).await;

                info!("Initiating Active Bank self-flash and system reset!");
                unsafe {
                    copy_and_reset(start_addr, 0x1000, content_len as u32);
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

// LED Blinky task for visual debugging
#[embassy_executor::task]
async fn blink_task(pin: Peri<'static, peripherals::P0_15>) -> ! {
    use embassy_nrf::gpio::{Level, Output, OutputDrive};
    use embassy_time::Timer;

    let mut led = Output::new(pin, Level::High, OutputDrive::Standard);
    loop {
        led.set_low(); // ON
        Timer::after(Duration::from_millis(50)).await;
        led.set_high(); // OFF
        Timer::after(Duration::from_millis(1950)).await;
    }
}

#[repr(align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    // Spawn the blink task immediately so we get visual feedback
    spawner.spawn(unwrap!(blink_task(p.P0_15)));

    // Generate random seed from hardware RNG
    let mut rng = embassy_nrf::rng::Rng::new_blocking(p.RNG);
    let mut seed_bytes = [0u8; 8];
    rng.blocking_fill_bytes(&mut seed_bytes);
    let seed = u64::from_le_bytes(seed_bytes);

    // Initialize VBUS detect using the hardware Power peripheral
    let vbus_detect = HardwareVbusDetect::new(Irqs);
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    // Configure the USB device stack
    let mut config = Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Embassy");
    config.product = Some("nice!nano v2 Web Server");
    config.serial_number = Some("12345678");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Static cells for aligned descriptors and state buffers
    static DEVICE_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONFIG_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONTROL_BUF: StaticCell<AlignedBuffer<128>> = StaticCell::new();

    let device_desc = &mut DEVICE_DESCRIPTOR.init(AlignedBuffer { data: [0; 256] }).data;
    let config_desc = &mut CONFIG_DESCRIPTOR.init(AlignedBuffer { data: [0; 256] }).data;
    let bos_desc = &mut BOS_DESCRIPTOR.init(AlignedBuffer { data: [0; 256] }).data;
    let control_buf = &mut CONTROL_BUF.init(AlignedBuffer { data: [0; 128] }).data;

    let mut builder = Builder::new(
        driver,
        config,
        device_desc,
        config_desc,
        bos_desc,
        control_buf,
    );

    // Define MAC addresses (Host MAC is 8c, Device MAC is d0)
    let host_mac_addr = [0x88, 0x88, 0x88, 0x88, 0x88, 0x8c];
    let our_mac_addr = [0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xd0];

    // Initialize CDC-NCM class
    static STATE: StaticCell<State> = StaticCell::new();
    let class = CdcNcmClass::new(&mut builder, STATE.init(State::new()), host_mac_addr, 64);

    let usb = builder.build();

    // Spawn USB device task
    spawner.spawn(unwrap!(usb_task(usb)));

    // Split NCM class into net device and runner
    static NET_STATE: StaticCell<NetState<MTU, 4, 4>> = StaticCell::new();
    let (runner, device) = class.into_embassy_net_device::<MTU, 4, 4>(NET_STATE.init(NetState::new()), our_mac_addr);

    // Spawn NCM runner task
    spawner.spawn(unwrap!(usb_ncm_task(runner)));

    // Configure static IPv4 parameters for the board
    use embassy_net::{Ipv4Address, Ipv4Cidr, StaticConfigV4};
    let config = StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 42, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 42, 1)),
        dns_servers: heapless::Vec::<Ipv4Address, 3>::new(),
    };

    // Initialize the TCP/IP stack
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();

    let (stack, net_runner) = embassy_net::new(
        device,
        embassy_net::Config::ipv4_static(config),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    // Spawn TCP/IP stack task
    spawner.spawn(unwrap!(net_task(net_runner)));

    // Spawn DHCP server task
    spawner.spawn(unwrap!(dhcp_task(stack)));

    // Spawn Web server task
    spawner.spawn(unwrap!(web_task(stack)));
}
