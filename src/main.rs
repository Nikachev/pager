#![no_std]
#![no_main]

mod ble;
mod flash;
mod web;

use core::cell::RefCell;
use defmt::*;
use defmt_rtt as _;
use panic_probe as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::mode::Async;
use embassy_nrf::peripherals::RNG;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, rng, usb};
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State as AcmState};
use embassy_usb::class::cdc_ncm::embassy_net::{Device as NetDevice, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::{Builder, Config};
use embassy_net::{Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embedded_io_async::Write;
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;
use trouble_host::prelude::*;
use ble::Server;

bind_interrupts!(struct Irqs {
    RNG => rng::InterruptHandler<RNG>;
    EGU0_SWI0 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    RTC0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    USBD => usb::InterruptHandler<embassy_nrf::peripherals::USBD>;
});

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[embassy_executor::task]
async fn blink_task(mut led: Output<'static>) -> ! {
    let mut mode = 0;
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

const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut rng::Rng<Async>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_adv()
        .support_peripheral()
        .peripheral_count(1)?
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            L2CAP_TXQ,
            L2CAP_RXQ,
        )?
        .build(p, rng, mpsl, mem)
}

pub const MTU: usize = 1514;
pub const IP_ADDRESS: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const GATEWAY: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const SUBNET_MASK: Ipv4Address = Ipv4Address::new(255, 255, 255, 0);
pub const DNS_SERVER: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const DHCP_POOL_START: Ipv4Address = Ipv4Address::new(192, 168, 42, 10);
pub const DHCP_POOL_END: Ipv4Address = Ipv4Address::new(192, 168, 42, 50);

pub const USB_VENDOR_ID: u16 = 0x1209;
pub const USB_PRODUCT_ID: u16 = 0x0001;
pub const USB_MANUFACTURER: &'static str = "Antigravity";
pub const USB_PRODUCT_NAME: &'static str = "Pager NCM+ACM";
pub const USB_SERIAL_NUMBER: &'static str = "12345678";

pub const HOST_MAC_ADDR: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
pub const DEVICE_MAC_ADDR: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];

type MyDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[repr(C, align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

pub static LOG_CHANNEL: Channel<ThreadModeRawMutex, heapless::String<128>, 128> = Channel::new();
pub static LOG_HISTORY: SyncMutex<ThreadModeRawMutex, RefCell<heapless::Vec<heapless::String<128>, 64>>> =
    SyncMutex::new(RefCell::new(heapless::Vec::new()));

pub fn get_logs() -> heapless::Vec<heapless::String<128>, 64> {
    LOG_HISTORY.lock(|hist| hist.borrow().clone())
}

pub static LED_MODE: embassy_sync::signal::Signal<ThreadModeRawMutex, u8> = embassy_sync::signal::Signal::new();

#[macro_export]
macro_rules! log_msg {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        let mut s = heapless::String::<128>::new();
        let _ = core::fmt::write(&mut s, format_args!($($arg)*));
        let _ = $crate::LOG_CHANNEL.try_send(s.clone());
        $crate::LOG_HISTORY.lock(|hist| {
            let mut h = hist.borrow_mut();
            if h.is_full() {
                h.remove(0);
            }
            let _ = h.push(s);
        });
    }};
}

#[embassy_executor::task]
async fn vbus_detect_task(vbus_detect: &'static SoftwareVbusDetect) -> ! {
    loop {
        vbus_detect.detected(true);
        vbus_detect.ready();
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn usb_task(mut usb: embassy_usb::UsbDevice<'static, MyDriver>) -> ! {
    usb.run().await
}

#[embassy_executor::task]
async fn usb_ncm_task(runner: embassy_usb::class::cdc_ncm::embassy_net::Runner<'static, MyDriver, MTU>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, NetDevice<'static, MTU>>) -> ! {
    runner.run().await
}

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

#[embassy_executor::task]
async fn usb_logger_task(mut sender: Sender<'static, MyDriver>) -> ! {
    let _ = sender.write_all(b"nice!nano v2 serial logger started.\r\n").await;
    loop {
        let msg = LOG_CHANNEL.receive().await;
        let _ = sender.write_all(msg.as_bytes()).await;
        let _ = sender.write_all(b"\r\n").await;
    }
}

#[embassy_executor::task]
async fn heartbeat_task() -> ! {
    let mut count = 0;
    loop {
        Timer::after(Duration::from_secs(1)).await;
        count += 1;
        crate::log_msg!("System heartbeat uptime: {}s", count);
    }
}

#[embassy_executor::task]
async fn usb_receiver_task(
    mut receiver: Receiver<'static, MyDriver>,
    flash_mutex: &'static Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>,
) -> ! {
    let mut cmd_buf = [0u8; 128];
    let mut cmd_len = 0;

    loop {
        let mut buf = [0u8; 64];
        match receiver.read_packet(&mut buf).await {
            Ok(n) if n > 0 => {
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if cmd_len > 0 {
                            let cmd_str = core::str::from_utf8(&cmd_buf[..cmd_len]).unwrap_or("").trim();
                            cmd_len = 0;

                            if cmd_str == "bootloader" || cmd_str == "dfu" || cmd_str == "reboot" {
                                log_msg!("Rebooting device...");
                                Timer::after(Duration::from_millis(100)).await;
                                cortex_m::peripheral::SCB::sys_reset();
                            } else if cmd_str.starts_with("update") {
                                let len_str = if cmd_str.len() > 6 { cmd_str[6..].trim() } else { "" };
                                let content_len: usize = match len_str.parse() {
                                    Ok(l) if l > 0 && l <= web::MAX_BIN_SIZE => l,
                                    _ => {
                                        log_msg!("SERIAL_UPDATE:ERROR_INVALID_SIZE");
                                        continue;
                                    }
                                };
                                log_msg!("SERIAL_UPDATE:START:{}", content_len);
                                let mut flash = flash_mutex.lock().await;
                                let mut writer = crate::flash::OtaWriter::new(&mut *flash, web::STAGING_START_ADDR);

                                let mut total_read = 0;
                                let mut chunk_buf = [0u8; 64];
                                while total_read < content_len {
                                    match receiver.read_packet(&mut chunk_buf).await {
                                        Ok(cn) if cn > 0 => {
                                            let to_write = core::cmp::min(cn, content_len - total_read);
                                            let _ = writer.write_chunk(&chunk_buf[..to_write]).await;
                                            total_read += to_write;
                                        }
                                        _ => {
                                            Timer::after(Duration::from_millis(1)).await;
                                        }
                                    }
                                }
                                let _ = writer.flush().await;
                                log_msg!("SERIAL_UPDATE:COMPLETE");
                                Timer::after(Duration::from_millis(500)).await;
                                unsafe {
                                    crate::flash::copy_and_reset(web::STAGING_START_ADDR, web::ACTIVE_START_ADDR, content_len as u32);
                                }
                            } else if cmd_str.starts_with("type ") {
                                let text = &cmd_str[5..];
                                log_msg!("Serial command type: {}", text);
                                let mut s = heapless::String::<128>::new();
                                let _ = s.push_str(text);
                                let _ = ble::BLE_COMMANDS.try_send(ble::BleCommand::TypeString(s));
                            } else if cmd_str == "pair" {
                                log_msg!("Serial command: Triggering pairing mode");
                                let _ = ble::BLE_COMMANDS.try_send(ble::BleCommand::RestartAdvertising);
                            } else if cmd_str == "clear_bonds" {
                                log_msg!("Serial command: Clearing persistent bonds");
                                let mut flash = flash_mutex.lock().await;
                                ble::erase_bond_slot(&mut *flash, 0).await;
                            }
                        }
                    } else if cmd_len < cmd_buf.len() {
                        cmd_buf[cmd_len] = b;
                        cmd_len += 1;
                    }
                }
            }
            _ => {
                Timer::after(Duration::from_millis(100)).await;
            }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = Priority::P2;
    config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(config);

    embassy_nrf::interrupt::USBD.set_priority(Priority::P2);

    // --- MPSL & SDC ---
    let mpsl_p = mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
        rc_ctiv: 0,
        rc_temp_ctiv: 0,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk_cfg)));
    spawner.spawn(unwrap!(mpsl_task(&*mpsl)));

    // Request HFXO (High-Frequency External Crystal Oscillator - 32MHz) via MPSL for USB PHY clock
    let hfclk = unwrap!(mpsl.request_hfclk().await);
    unwrap!(nrf_mpsl::Hfclk::wait().await);
    core::mem::forget(hfclk); // Keep HFCLK active indefinitely for USB 48MHz clock

    // Spawn LED blink task
    let led = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    spawner.spawn(unwrap!(blink_task(led)));

    // Flash driver via nrf-mpsl
    let flash_driver = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    static FLASH_MUTEX: StaticCell<Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>> = StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(Mutex::new(flash_driver));

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24, p.PPI_CH25, p.PPI_CH26,
        p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    let mut rng = rng::Rng::new(p.RNG, Irqs);
    let mut sdc_mem = sdc::Mem::<4720>::new();
    let sdc = unwrap!(build_sdc(sdc_p, &mut rng, mpsl, &mut sdc_mem));

    // --- trouble host stack ---
    let address: Address = Address::random([0xda, 0x3c, 0xf3, 0x52, 0x35, 0xf1]);
    info!("TrouBLE-HID: our address = {:?}", address);

    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(sdc, &mut resources)
        .set_random_address(address)
        .build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "TrouBLE-Pager",
        appearance: &appearance::human_interface_device::GENERIC_HUMAN_INTERFACE_DEVICE,
    }))
    .unwrap();

    let mut adv_data = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids16(&[[0x12, 0x18]]),
            AdStructure::CompleteLocalName(b"TrouBLE-Pager"),
        ],
        &mut adv_data[..],
    )
    .unwrap();

    // Force USB D+ pullup low for 1s to ensure host sees clean USB disconnect/reconnect
    unsafe { core::ptr::write_volatile(0x40027504 as *mut u32, 0); }
    Timer::after(Duration::from_millis(1000)).await;

    // --- USB Initialization ---
    static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus_detect: &'static SoftwareVbusDetect = &*VBUS_DETECT.init(SoftwareVbusDetect::new(true, true));
    vbus_detect.detected(true);
    vbus_detect.ready();
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    spawner.spawn(unwrap!(vbus_detect_task(vbus_detect)));

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

    static STATE: StaticCell<State> = StaticCell::new();
    let class = CdcNcmClass::new(&mut builder, STATE.init(State::new()), HOST_MAC_ADDR, 64);

    static ACM_STATE: StaticCell<AcmState> = StaticCell::new();
    let acm_class = CdcAcmClass::new(&mut builder, ACM_STATE.init(AcmState::new()), 64);

    let usb = builder.build();
    let (acm_sender, acm_receiver) = acm_class.split();

    spawner.spawn(unwrap!(usb_task(usb)));
    spawner.spawn(unwrap!(usb_logger_task(acm_sender)));
    spawner.spawn(unwrap!(usb_receiver_task(acm_receiver, flash_mutex)));

    static NET_STATE: StaticCell<NetState<MTU, 4, 4>> = StaticCell::new();
    let (net_runner, device) = class.into_embassy_net_device::<MTU, 4, 4>(NET_STATE.init(NetState::new()), DEVICE_MAC_ADDR);

    spawner.spawn(unwrap!(usb_ncm_task(net_runner)));

    let net_config = StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 42, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 42, 1)),
        dns_servers: heapless::Vec::<Ipv4Address, 3>::new(),
    };

    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let seed = 0x1234_5678_9ABC_DEF0u64;

    let (stack_net, runner_net) = embassy_net::new(
        device,
        embassy_net::Config::ipv4_static(net_config),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(unwrap!(net_task(runner_net)));
    spawner.spawn(unwrap!(dhcp_task(stack_net)));
    spawner.spawn(unwrap!(web::web_task(stack_net, flash_mutex)));
    spawner.spawn(unwrap!(heartbeat_task()));

    info!("TrouBLE-Pager HID & Web Server: starting advertising");
    let _ = embassy_futures::join::join(runner.run(), async {
        loop {
            let advertiser = peripheral
                .advertise(
                    &Default::default(),
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &adv_data[..len],
                        scan_data: &[],
                    },
                )
                .await
                .unwrap();
            info!("TrouBLE-Pager: waiting for connection...");
            let conn = advertiser.accept().await.unwrap().with_attribute_server(&server).unwrap();
            info!("TrouBLE-Pager: connection established!");
            let _ = conn.raw().set_bondable(true);

            loop {
                match conn.next().await {
                    GattConnectionEvent::Disconnected { reason } => {
                        info!("TrouBLE-Pager: disconnected {:?}", reason);
                        break;
                    }
                    GattConnectionEvent::PairingComplete { security_level, bond } => {
                        info!("TrouBLE-Pager: pairing complete! Level: {:?}, Bond: {:?}", security_level, bond);
                    }
                    GattConnectionEvent::PairingFailed(err) => {
                        warn!("TrouBLE-Pager: pairing failed: {:?}", err);
                    }
                    GattConnectionEvent::Gatt { event } => {
                        match event {
                            GattEvent::Read(req) => {
                                if let Ok(reply) = req.accept() {
                                    let _ = reply.send().await;
                                }
                            }
                            GattEvent::Write(req) => {
                                if let Ok(reply) = req.accept() {
                                    let _ = reply.send().await;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    })
    .await;
}
