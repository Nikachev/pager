#![no_std]
#![no_main]

mod flash;
mod web;

use defmt::*;
use embassy_executor::Spawner;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, peripherals, Peri};
use embassy_usb::class::cdc_ncm::embassy_net::{Device, Runner, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::{Builder, Config, UsbDevice};
use embassy_net::{Stack, StackResources, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use core::net::Ipv4Addr;
use {defmt_rtt as _, panic_probe as _};

// Bind interrupts for the USB controller and the Power controller (VBUS detection)
bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

// Network Configuration Constants
const IP_ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 42, 1);
const SUBNET_MASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 42, 1);
const DNS_SERVER: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
const DHCP_POOL_START: Ipv4Addr = Ipv4Addr::new(192, 168, 42, 2);
const DHCP_POOL_END: Ipv4Addr = Ipv4Addr::new(192, 168, 42, 10);

// Hardware MAC Configuration Constants
const HOST_MAC_ADDR: [u8; 6] = [0x88, 0x88, 0x88, 0x88, 0x88, 0x92];
const DEVICE_MAC_ADDR: [u8; 6] = [0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xd6];

// USB Stack Configuration Constants
const USB_VENDOR_ID: u16 = 0xc0de;
const USB_PRODUCT_ID: u16 = 0xcafe;
const USB_MANUFACTURER: &str = "Embassy";
const USB_PRODUCT_NAME: &str = "nice!nano v2 Web Server";
const USB_SERIAL_NUMBER: &str = "12345680";

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
    use leasehund::DhcpServer;

    let mut server = DhcpServer::<32, 4>::new(
        IP_ADDRESS,
        SUBNET_MASK,
        GATEWAY,
        DNS_SERVER,
        DHCP_POOL_START,
        DHCP_POOL_END,
    );

    info!("DHCP server started. Waiting for requests on port 67...");
    server.run(stack).await;
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
    // Configure HFXO high-frequency crystal oscillator for precise, stable USB timing on boot
    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);

    // Spawn the blink task immediately so we get visual feedback
    spawner.spawn(unwrap!(blink_task(p.P0_15)));

    // Delay USB initialization to ensure the host registers a clean disconnect-reconnect event
    Timer::after(Duration::from_secs(2)).await;

    // Generate random seed from hardware RNG
    let mut rng = embassy_nrf::rng::Rng::new_blocking(p.RNG);
    let mut seed_bytes = [0u8; 8];
    rng.blocking_fill_bytes(&mut seed_bytes);
    let seed = u64::from_le_bytes(seed_bytes);

    // Initialize hardware VBUS detect
    let vbus_detect = HardwareVbusDetect::new(Irqs);
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    // Configure the USB device stack
    let mut usb_config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT_NAME);
    usb_config.serial_number = Some(USB_SERIAL_NUMBER);
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

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
        usb_config,
        device_desc,
        config_desc,
        bos_desc,
        control_buf,
    );

    // Initialize CDC-NCM class
    static STATE: StaticCell<State> = StaticCell::new();
    let class = CdcNcmClass::new(&mut builder, STATE.init(State::new()), HOST_MAC_ADDR, 64);

    let usb = builder.build();

    // Spawn USB device task
    spawner.spawn(unwrap!(usb_task(usb)));

    // Split NCM class into net device and runner
    static NET_STATE: StaticCell<NetState<MTU, 4, 4>> = StaticCell::new();
    let (runner, device) = class.into_embassy_net_device::<MTU, 4, 4>(NET_STATE.init(NetState::new()), DEVICE_MAC_ADDR);

    // Spawn NCM runner task
    spawner.spawn(unwrap!(usb_ncm_task(runner)));

    // Configure static IPv4 parameters for the board
    let net_config = StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(IP_ADDRESS.octets()[0], IP_ADDRESS.octets()[1], IP_ADDRESS.octets()[2], IP_ADDRESS.octets()[3]), 24),
        gateway: Some(Ipv4Address::new(GATEWAY.octets()[0], GATEWAY.octets()[1], GATEWAY.octets()[2], GATEWAY.octets()[3])),
        dns_servers: heapless::Vec::<Ipv4Address, 3>::new(),
    };

    // Initialize the TCP/IP stack
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();

    let (stack, net_runner) = embassy_net::new(
        device,
        embassy_net::Config::ipv4_static(net_config),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    // Spawn TCP/IP stack task
    spawner.spawn(unwrap!(net_task(net_runner)));

    // Spawn DHCP server task
    spawner.spawn(unwrap!(dhcp_task(stack)));

    // Spawn Web server task from the web module
    spawner.spawn(unwrap!(web::web_task(stack)));
}
