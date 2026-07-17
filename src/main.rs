#![no_std]
#![no_main]

mod flash;
mod web;
mod ble;

use defmt::{info, warn, error, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, peripherals, Peri};
use embassy_usb::class::cdc_ncm::embassy_net::{Device, Runner, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::{Builder, Config, UsbDevice};
use embassy_net::{Stack, StackResources, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use core::net::Ipv4Addr;
use defmt_rtt as _;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use nrf_softdevice::raw;

pub static LED_MODE: Signal<ThreadModeRawMutex, u8> = Signal::new();



// Bind interrupts for the USB controller
bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
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
type MyDriver = Driver<'static, &'static SoftwareVbusDetect>;

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

// LED Blinky task for visual debugging and BLE control
#[embassy_executor::task]
async fn blink_task(pin: Peri<'static, peripherals::P0_15>) -> ! {
    use embassy_nrf::gpio::{Level, Output, OutputDrive};
    use embassy_time::Timer;

    let mut led = Output::new(pin, Level::High, OutputDrive::Standard);
    let mut mode = 0; // 0 = Auto blink, 1 = Manual OFF, 2 = Manual ON

    loop {
        match mode {
            0 => {
                led.set_low(); // ON
                match embassy_futures::select::select(Timer::after(Duration::from_millis(50)), LED_MODE.wait()).await {
                    embassy_futures::select::Either::First(_) => {
                        led.set_high(); // OFF
                        match embassy_futures::select::select(Timer::after(Duration::from_millis(1950)), LED_MODE.wait()).await {
                            embassy_futures::select::Either::First(_) => {}
                            embassy_futures::select::Either::Second(new_mode) => {
                                mode = new_mode;
                            }
                        }
                    }
                    embassy_futures::select::Either::Second(new_mode) => {
                        mode = new_mode;
                    }
                }
            }
            1 => {
                led.set_high(); // OFF
                mode = LED_MODE.wait().await;
            }
            2 => {
                led.set_low(); // ON
                mode = LED_MODE.wait().await;
            }
            _ => {
                mode = 0;
            }
        }
    }
}

#[repr(align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

#[embassy_executor::task]
async fn softdevice_task(sd: &'static nrf_softdevice::Softdevice) -> ! {
    sd.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Configure embassy-nrf defaults first
    let mut config = embassy_nrf::config::Config::default();
    // SoftDevice reserves interrupt priority levels 0, 1, 4, and 5.
    // We must move HAL interrupts to P2 or lower (P2, P3, P6, P7) to avoid panic.
    config.gpiote_interrupt_priority = embassy_nrf::interrupt::Priority::P2;
    config.time_interrupt_priority = embassy_nrf::interrupt::Priority::P2;
    let p = embassy_nrf::init(config);

    // Set USBD interrupt priority to P2 to prevent SoftDevice conflicts (recovers USB transfer capability)
    unsafe {
        use embassy_nrf::interrupt::InterruptExt;
        embassy_nrf::interrupt::USBD.set_priority(embassy_nrf::interrupt::Priority::P2);
    }

    // Configure LFCLK RC oscillator clock for SoftDevice (guarantees boot on all nice!nano variants)
    let sd_config = nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        conn_gap: Some(raw::ble_gap_conn_cfg_t {
            conn_count: 1,
            event_length: 24,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 128 }),
        gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
            attr_tab_size: 1408,
        }),
        ..Default::default()
    };

    let sd = nrf_softdevice::Softdevice::enable(&sd_config);

    let server = unwrap!(ble::Server::new(&mut *sd));
    
    // Downgrade to shared static reference for sharing between tasks
    let sd: &'static nrf_softdevice::Softdevice = &*sd;
    
    spawner.spawn(unwrap!(softdevice_task(sd)));

    // Spawn the blink task immediately so we get visual feedback
    spawner.spawn(unwrap!(blink_task(p.P0_15)));

    // Spawn BLE task
    static SERVER: StaticCell<ble::Server> = StaticCell::new();
    let server_ref = SERVER.init(server);
    spawner.spawn(unwrap!(ble::ble_task(sd, server_ref)));

    // Delay USB initialization to ensure the host registers a clean disconnect-reconnect event
    Timer::after(Duration::from_secs(2)).await;

    // Generate random seed from SoftDevice RNG (as hardware RNG is protected/blocked by SoftDevice)
    let mut seed_bytes = [0u8; 8];
    unwrap!(nrf_softdevice::random_bytes(sd, &mut seed_bytes));
    let seed = u64::from_le_bytes(seed_bytes);

    // Request HFXO (external 32MHz crystal) from SoftDevice for USB precise timing
    unwrap!(nrf_softdevice::RawError::convert(unsafe { raw::sd_clock_hfclk_request() }));

    // Wait for HFXO to be fully running and stable
    loop {
        let mut is_running: u32 = 0;
        let err = unsafe { raw::sd_clock_hfclk_is_running(&mut is_running) };
        if err == 0 && is_running != 0 {
            break;
        }
        Timer::after(Duration::from_millis(5)).await;
    }

    // Initialize software VBUS detect (always present when active)
    static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus_detect: &'static SoftwareVbusDetect = &*VBUS_DETECT.init(SoftwareVbusDetect::new(true, true));
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

    // Initialize SoftDevice safe Flash driver
    let flash = nrf_softdevice::Flash::take(sd);
    static FLASH: StaticCell<nrf_softdevice::Flash> = StaticCell::new();
    let flash_ref = FLASH.init(flash);

    // Spawn Web server task from the web module
    spawner.spawn(unwrap!(web::web_task(stack, flash_ref)));
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Write defmt message if possible
    error!("PANIC: {:?}", defmt::Debug2Format(info));

    // Force P0.15 (LED) to output mode via direct register write
    unsafe {
        let pin_cnf = 0x5000073C as *mut u32;
        *pin_cnf = 1; // Dir: output
    }

    loop {
        // Toggle LED fast for visual panic notification (100ms ON / 100ms OFF)
        unsafe {
            let outclr = 0x5000050C as *mut u32;
            *outclr = 1 << 15; // LED ON (Low)
        }
        cortex_m::asm::delay(8_000_000);
        unsafe {
            let outset = 0x50000508 as *mut u32;
            *outset = 1 << 15; // LED OFF (High)
        }
        cortex_m::asm::delay(8_000_000);
    }
}
