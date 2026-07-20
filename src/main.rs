#![no_std]
#![no_main]

mod flash;
mod web;
mod ble;

use defmt::{error, unwrap};
use embassy_executor::Spawner;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, peripherals, Peri};
use embassy_usb::class::cdc_ncm::embassy_net::{Device, Runner, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State as AcmState};
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
use embedded_io_async::Write;

pub static LED_MODE: Signal<ThreadModeRawMutex, u8> = Signal::new();

pub static LOG_CHANNEL: embassy_sync::channel::Channel<ThreadModeRawMutex, heapless::String<128>, 128> = embassy_sync::channel::Channel::new();

pub struct LogHistory {
    lines: heapless::Vec<heapless::String<96>, 64>,
}

impl LogHistory {
    pub const fn new() -> Self {
        Self {
            lines: heapless::Vec::new(),
        }
    }

    pub fn push(&mut self, line: &str) {
        let trimmed = line.trim_end();
        let limit = core::cmp::min(trimmed.len(), 95);
        let mut s = heapless::String::new();
        if s.push_str(&trimmed[..limit]).is_ok() {
            if self.lines.is_full() {
                self.lines.remove(0);
            }
            let _ = self.lines.push(s);
        }
    }
}

pub static LOG_HISTORY: embassy_sync::blocking_mutex::Mutex<ThreadModeRawMutex, core::cell::RefCell<LogHistory>> =
    embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new(LogHistory::new()));

pub fn log_to_history(s: &str) {
    LOG_HISTORY.lock(|cell| {
        cell.borrow_mut().push(s);
    });
}

    pub fn get_logs() -> heapless::Vec<heapless::String<96>, 64> {
    LOG_HISTORY.lock(|cell| {
        cell.borrow().lines.clone()
    })
}

#[macro_export]
macro_rules! log_msg {
    ($($arg:tt)*) => {
        {
            // Log to RTT/defmt
            defmt::info!($($arg)*);
            
            // Format and log to USB serial
            let mut s = heapless::String::<128>::new();
            if core::fmt::write(&mut s, format_args!($($arg)*)).is_ok() {
                let _ = s.push_str("\r\n");
                let _ = $crate::LOG_CHANNEL.try_send(s.clone());
                $crate::log_to_history(&s);
            }
        }
    };
}

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

    crate::log_msg!("DHCP server started. Waiting for requests on port 67...");
    server.run(stack).await;
}
// USB CDC-ACM Logger task
#[embassy_executor::task]
async fn usb_logger_task(mut sender: embassy_usb::class::cdc_acm::Sender<'static, MyDriver>) -> ! {
    loop {
        sender.wait_connection().await;
        let _ = sender.write_all(b"nice!nano v2 serial logger started.\r\n").await;
        loop {
            let msg = LOG_CHANNEL.receive().await;
            if let Err(_e) = sender.write_all(msg.as_bytes()).await {
                break; // Host disconnected
            }
        }
    }
}

fn starts_with(data: &[u8], prefix: &[u8]) -> bool {
    data.len() >= prefix.len() && &data[..prefix.len()] == prefix
}

// USB CDC-ACM Receiver task to reset to DFU/Bootloader
#[embassy_executor::task]
async fn usb_receiver_task(
    mut receiver: embassy_usb::class::cdc_acm::Receiver<'static, MyDriver>,
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_softdevice::Flash>,
) -> ! {
    let mut buf = [0u8; 64];
    loop {
        receiver.wait_connection().await;
        loop {
            match receiver.read_packet(&mut buf).await {
                Ok(0) => {
                    // Host disconnected (zero-length read). Drop back to
                    // waiting for a fresh connection instead of spinning.
                    break;
                }
                Ok(n) => {
                    let cmd = &buf[..n];
                    if starts_with(cmd, b"bootloader") || starts_with(cmd, b"dfu") {
                        crate::log_msg!("Rebooting to Bootloader (UF2)...");
                        // Delay 100ms for log to send
                        embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
                        
                        unsafe {
                            let _ = nrf_softdevice::raw::sd_power_gpregret_set(0, 0x57);
                            let aircr = 0xE000ED0C as *mut u32;
                            core::ptr::write_volatile(aircr, 0x05FA0004);
                        }
                    } else if starts_with(cmd, b"update ") {
                        // Parse size
                        let mut size = 0;
                        let mut valid = false;
                        for &c in &cmd[7..] {
                            if c >= b'0' && c <= b'9' {
                                size = size * 10 + (c - b'0') as usize;
                                valid = true;
                            } else if c == b'\r' || c == b'\n' || c == b' ' {
                                break;
                            } else {
                                valid = false;
                                break;
                            }
                        }

                        if valid && size > 0 && size <= crate::web::MAX_BIN_SIZE {
                            crate::log_msg!("SERIAL_UPDATE:READY");
                            
                            let mut write_error = false;
                            let mut total_read = 0;
                            
                            // Lock flash
                            let mut flash = flash_mutex.lock().await;
                            let mut writer = crate::flash::OtaWriter::new(&mut *flash, crate::web::STAGING_START_ADDR);
                            
                            if let Err(_e) = writer.erase(size).await {
                                crate::log_msg!("SERIAL_UPDATE:ERROR_ERASE");
                                write_error = true;
                            }
                            
                            while !write_error && total_read < size {
                                let mut read_buf = [0u8; 64];
                                match receiver.read_packet(&mut read_buf).await {
                                    Ok(n_pack) => {
                                        if n_pack > 0 {
                                            let chunk = &read_buf[..n_pack];
                                            if let Err(_e) = writer.write_chunk(chunk).await {
                                                crate::log_msg!("SERIAL_UPDATE:ERROR_WRITE");
                                                write_error = true;
                                                break;
                                            }
                                            total_read += n_pack;
                                        }
                                    }
                                    Err(_e) => {
                                        crate::log_msg!("SERIAL_UPDATE:ERROR_DISCONNECT");
                                        write_error = true;
                                        break;
                                    }
                                }
                            }
                            
                            if !write_error && total_read == size {
                                if let Err(_e) = writer.flush().await {
                                    crate::log_msg!("SERIAL_UPDATE:ERROR_FLUSH");
                                } else {
                                    crate::log_msg!("SERIAL_UPDATE:SUCCESS");
                                    // Wait 500ms and reset
                                    embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
                                    unsafe {
                                        crate::flash::copy_and_reset(
                                            crate::web::STAGING_START_ADDR,
                                            crate::web::ACTIVE_START_ADDR,
                                            size as u32,
                                        );
                                    }
                                }
                            }
                        } else {
                            crate::log_msg!("SERIAL_UPDATE:ERROR_INVALID_SIZE");
                        }
                    }
                }
                Err(_e) => {
                    break; // Host disconnected
                }
            }
        }
    }
}

// USB VBUS detection task to dynamically enable/disable USB stack
#[embassy_executor::task]
async fn vbus_detect_task(vbus_detect: &'static SoftwareVbusDetect) -> ! {
    let usbregstatus = 0x40000438 as *const u32;
    let mut last_detected = true;
    let mut last_ready = true;

    loop {
        let status = unsafe { core::ptr::read_volatile(usbregstatus) };
        let detected = (status & 1) != 0;
        let ready = (status & 2) != 0;

        if detected != last_detected {
            vbus_detect.detected(detected);
            last_detected = detected;
        }

        if ready != last_ready {
            if ready {
                vbus_detect.ready();
            }
            last_ready = ready;
        }

        embassy_time::Timer::after(embassy_time::Duration::from_millis(100)).await;
    }
}

#[embassy_executor::task]
async fn blink_task(pin: Peri<'static, peripherals::P0_15>) -> ! {
    use embassy_nrf::gpio::{Level, Output, OutputDrive};
    use embassy_time::Timer;

    let mut led = Output::new(pin, Level::High, OutputDrive::Standard);
    let mut mode = 0; // 0 = Auto blink, 1 = Manual OFF, 2 = Manual ON

    let mut count = 0;
    loop {
        count += 1;
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
    use embassy_nrf::interrupt::InterruptExt;
    embassy_nrf::interrupt::USBD.set_priority(embassy_nrf::interrupt::Priority::P2);

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

    // Initialize SoftDevice safe Flash driver wrapped in a Mutex
    let flash = nrf_softdevice::Flash::take(sd);
    static FLASH: StaticCell<embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_softdevice::Flash>> = StaticCell::new();
    let flash_mutex = FLASH.init(embassy_sync::mutex::Mutex::new(flash));
    
    spawner.spawn(unwrap!(softdevice_task(sd)));

    // Spawn the blink task immediately so we get visual feedback
    spawner.spawn(unwrap!(blink_task(p.P0_15)));

    // Spawn BLE task
    static SERVER: StaticCell<ble::Server> = StaticCell::new();
    let server_ref = SERVER.init(server);
    spawner.spawn(unwrap!(ble::ble_task(sd, server_ref, flash_mutex)));
    spawner.spawn(unwrap!(ble::bond_persist_task(flash_mutex)));

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

    // Initialize software VBUS detect (updated dynamically by vbus_detect_task)
    static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus_detect: &'static SoftwareVbusDetect = &*VBUS_DETECT.init(SoftwareVbusDetect::new(true, true));
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    // Spawn VBUS detection task to dynamically update connection state
    spawner.spawn(unwrap!(vbus_detect_task(vbus_detect)));

    // Configure the USB device stack
    let mut usb_config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT_NAME);
    usb_config.serial_number = Some(USB_SERIAL_NUMBER);
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;
    usb_config.device_class = 0xEF;
    usb_config.device_sub_class = 0x02;
    usb_config.device_protocol = 0x01;
    usb_config.composite_with_iads = true;

    // Static cells for aligned descriptors and state buffers
    static DEVICE_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONFIG_DESCRIPTOR: StaticCell<AlignedBuffer<512>> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONTROL_BUF: StaticCell<AlignedBuffer<128>> = StaticCell::new();

    let device_desc = &mut DEVICE_DESCRIPTOR.init(AlignedBuffer { data: [0; 256] }).data;
    let config_desc = &mut CONFIG_DESCRIPTOR.init(AlignedBuffer { data: [0; 512] }).data;
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

    // Initialize CDC-ACM class
    static ACM_STATE: StaticCell<AcmState> = StaticCell::new();
    let acm_class = CdcAcmClass::new(&mut builder, ACM_STATE.init(AcmState::new()), 64);

    let usb = builder.build();

    // Split ACM class
    let (acm_sender, acm_receiver) = acm_class.split();

    // Spawn USB device task
    spawner.spawn(unwrap!(usb_task(usb)));

    // Spawn USB logger task
    spawner.spawn(unwrap!(usb_logger_task(acm_sender)));

    // Spawn USB receiver task
    spawner.spawn(unwrap!(usb_receiver_task(acm_receiver, flash_mutex)));

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
    spawner.spawn(unwrap!(web::web_task(stack, flash_mutex)));
}

const P0_PIN_CNF_15: *mut u32 = 0x5000073C as *mut u32;
const P0_OUTCLR: *mut u32 = 0x5000050C as *mut u32;
const P0_OUTSET: *mut u32 = 0x50000508 as *mut u32;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Write defmt message if possible
    error!("PANIC: {:?}", defmt::Debug2Format(info));

    // Force P0.15 (LED) to output mode via direct register write
    unsafe {
        core::ptr::write_volatile(P0_PIN_CNF_15, 1); // Dir: output
    }

    loop {
        // Toggle LED fast for visual panic notification (100ms ON / 100ms OFF)
        unsafe {
            core::ptr::write_volatile(P0_OUTCLR, 1 << 15); // LED ON (Low)
        }
        cortex_m::asm::delay(8_000_000);
        unsafe {
            core::ptr::write_volatile(P0_OUTSET, 1 << 15); // LED OFF (High)
        }
        cortex_m::asm::delay(8_000_000);
    }
}

