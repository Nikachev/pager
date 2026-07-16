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

    // Set up the DHCP server on the USB Ethernet interface.
    // This will assign 192.168.42.2 to the laptop (host) when it requests an IP.
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

// Web server task serving the responsive HTML page on port 80
#[embassy_executor::task]
async fn web_task(stack: Stack<'static>) -> ! {
    use embassy_net::tcp::TcpSocket;
    use embedded_io_async::Write;

    let mut rx_buffer = [0u8; 2048];
    let mut tx_buffer = [0u8; 2048];
    let mut buf = [0u8; 1024];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("Web server listening on port 80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Connection accepted from {:?}", socket.remote_endpoint());

        // Read the incoming HTTP request header (and discard it)
        let _ = socket.read(&mut buf).await;

        // Build a highly aesthetic and modern HTML response
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

// LED Blinky task for visual debugging
#[embassy_executor::task]
async fn blink_task(pin: Peri<'static, peripherals::P0_15>) -> ! {
    use embassy_nrf::gpio::{Level, Output, OutputDrive};
    use embassy_time::Timer;

    // Blue status LED on nice!nano v2 is connected to P0.15 (Active Low)
    let mut led = Output::new(pin, Level::High, OutputDrive::Standard);
    loop {
        // Very short flash once every 2 seconds
        led.set_low(); // ON
        Timer::after(Duration::from_millis(50)).await;
        led.set_high(); // OFF
        Timer::after(Duration::from_millis(1950)).await;
    }
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

    // Static cells for descriptors and state buffers
    static DEVICE_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        DEVICE_DESCRIPTOR.init([0; 256]),
        CONFIG_DESCRIPTOR.init([0; 256]),
        BOS_DESCRIPTOR.init([0; 256]),
        CONTROL_BUF.init([0; 128]),
    );

    // Define MAC addresses
    let host_mac_addr = [0x88, 0x88, 0x88, 0x88, 0x88, 0x8a];
    let our_mac_addr = [0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xce];

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
