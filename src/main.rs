#![no_std]
#![no_main]

mod ble;
mod flash;
mod led;
mod protocol;
mod serial_task;
mod web;
mod webusb;
mod webusb_handler;

pub use led::{blink_task, LED_MODE};
pub use serial_task::{usb_logger_task, usb_receiver_task};
pub use webusb_handler::{webusb_reply, webusb_task};

use ble::Server;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::mode::Async;
use embassy_nrf::peripherals::RNG;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, rng, usb};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State as AcmState};
use embassy_usb::{Builder, Config};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use panic_probe as _;
use static_cell::StaticCell;
use trouble_host::prelude::*;

bind_interrupts!(struct Irqs {
    RNG => rng::InterruptHandler<RNG>;
    EGU0_SWI0 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    RTC0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    USBD => usb::InterruptHandler<embassy_nrf::peripherals::USBD>;
});

pub type MyDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

pub static LOG_CHANNEL: Channel<ThreadModeRawMutex, heapless::String<128>, 32> = Channel::new();
pub static DROPPED_LOGS: AtomicU32 = AtomicU32::new(0);

#[repr(C, align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

type LogHistory = RefCell<heapless::Vec<heapless::String<128>, 32>>;
static LOG_HISTORY: SyncMutex<ThreadModeRawMutex, LogHistory> =
    SyncMutex::new(RefCell::new(heapless::Vec::new()));

pub fn get_logs() -> heapless::Vec<heapless::String<128>, 32> {
    LOG_HISTORY.lock(|hist| {
        let mut v = heapless::Vec::new();
        for item in hist.borrow().iter() {
            let _ = v.push(item.clone());
        }
        v
    })
}

#[macro_export]
macro_rules! log_msg {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        let mut s = heapless::String::<128>::new();
        let _ = core::fmt::write(&mut s, format_args!($($arg)*));
        if $crate::LOG_CHANNEL.try_send(s.clone()).is_err() {
            let _ = $crate::LOG_CHANNEL.try_receive();
            let _ = $crate::LOG_CHANNEL.try_send(s);
            $crate::DROPPED_LOGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }};
}

const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

const WDT_TASKS_START: *mut u32 = 0x4001_0000 as *mut u32;
const WDT_CRV: *mut u32 = 0x4001_0504 as *mut u32;
const WDT_RREN: *mut u32 = 0x4001_0508 as *mut u32;
const WDT_CONFIG: *mut u32 = 0x4001_050C as *mut u32;
const WDT_RR0: *mut u32 = 0x4001_0600 as *mut u32;
const WDT_RELOAD_MAGIC: u32 = 0x6E52_4635;
const WDT_RELOAD_TICKS: u32 = 10 * 32_768;

fn start_watchdog() {
    unsafe {
        core::ptr::write_volatile(WDT_CONFIG, 0b01);
        core::ptr::write_volatile(WDT_CRV, WDT_RELOAD_TICKS);
        core::ptr::write_volatile(WDT_RREN, 1);
        core::ptr::write_volatile(WDT_TASKS_START, 1);
        core::ptr::write_volatile(WDT_RR0, WDT_RELOAD_MAGIC);
    }
}

pub const HEARTBEAT_BLINK: u8 = 1 << 0;
pub static TASK_HEARTBEATS: AtomicU32 = AtomicU32::new(0);

pub fn signal_heartbeat(flag: u8) {
    TASK_HEARTBEATS.fetch_or(flag as u32, Ordering::Relaxed);
}

#[embassy_executor::task]
async fn watchdog_task() -> ! {
    loop {
        Timer::after(Duration::from_secs(2)).await;
        let mask = TASK_HEARTBEATS.swap(0, Ordering::Relaxed);
        if (mask & (HEARTBEAT_BLINK as u32)) != 0
            || crate::flash::IS_FLASHING.load(Ordering::Relaxed)
        {
            if option_env!("PAGER_SKIP_WATCHDOG_FEED") != Some("1") {
                unsafe { core::ptr::write_volatile(WDT_RR0, WDT_RELOAD_MAGIC) };
            }
        } else {
            defmt::warn!("Watchdog: missing task heartbeat mask {:x}", mask);
        }
    }
}

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

pub const USB_VENDOR_ID: u16 = 0x1209;
pub const USB_PRODUCT_ID: u16 = 0x0001;
pub const USB_MANUFACTURER: &str = "Nikachev";
pub const USB_PRODUCT_NAME: &str = "Pager WebUSB+ACM";
const FICR_DEVICEID0: *const u32 = 0x1000_0060 as *const u32;
const FICR_DEVICEID1: *const u32 = 0x1000_0064 as *const u32;

fn factory_usb_serial() -> heapless::String<16> {
    let low = unsafe { core::ptr::read_volatile(FICR_DEVICEID0) };
    let high = unsafe { core::ptr::read_volatile(FICR_DEVICEID1) };
    let mut serial = heapless::String::new();
    let _ = core::fmt::write(&mut serial, format_args!("{:08x}{:08x}", high, low));
    serial
}

fn ble_static_random_address() -> [u8; 6] {
    let low = unsafe { core::ptr::read_volatile(FICR_DEVICEID0) };
    let high = unsafe { core::ptr::read_volatile(FICR_DEVICEID1) };
    let mut addr = [
        (low & 0xFF) as u8,
        ((low >> 8) & 0xFF) as u8,
        ((low >> 16) & 0xFF) as u8,
        ((low >> 24) & 0xFF) as u8,
        (high & 0xFF) as u8,
        ((high >> 8) & 0xFF) as u8,
    ];
    addr[5] |= 0xC0;
    addr
}

#[embassy_executor::task]
async fn vbus_detect_task(vbus_detect: &'static SoftwareVbusDetect) -> ! {
    loop {
        vbus_detect.detected(true);
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn usb_task(mut usb: embassy_usb::UsbDevice<'static, MyDriver>) -> ! {
    usb.run().await
}

#[embassy_executor::task]
async fn heartbeat_task() -> ! {
    let mut count = 0;
    loop {
        Timer::after(Duration::from_secs(10)).await;
        count += 10;
        crate::log_msg!("System heartbeat uptime: {}s", count);
    }
}

#[embassy_executor::task]
async fn persist_keyboard_state_task(
    flash_mutex: &'static Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>,
) -> ! {
    loop {
        ble::PERSIST_STATE.wait().await;
        let (active_slot, bonds) = ble::KEYBOARD_STATE.lock(|state| {
            let state = state.borrow();
            (state.active_slot, state.bonds.clone())
        });
        let mut flash = flash_mutex.lock().await;
        match flash::save_persistent_state(&mut *flash, active_slot, &bonds).await {
            Ok(()) => {}
            Err(error) => {
                log_msg!("PERSIST_STATE:ERROR:{:?}", error);
            }
        }
    }
}

fn sync_active_bond<C: Controller>(stack: &trouble_host::Stack<'_, C, DefaultPacketPool>) {
    let mut identities = heapless::Vec::<Identity, 3>::new();
    stack.with_bond_information(|bonds| {
        for bond in bonds {
            let _ = identities.push(bond.identity);
        }
    });
    for identity in identities {
        let _ = stack.remove_bond_information(identity);
    }
    let active_bond = ble::KEYBOARD_STATE.lock(|state| {
        let state = state.borrow();
        state.bonds[state.active_slot].clone()
    });
    if let Some(bond) = active_bond {
        if stack.add_bond_information(bond).is_err() {
            crate::log_msg!("BLE:BOND_SYNC_ERROR");
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = Priority::P2;
    config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(config);

    start_watchdog();
    spawner.spawn(unwrap!(watchdog_task()));

    embassy_nrf::interrupt::USBD.set_priority(Priority::P2);

    let mpsl_p =
        mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
        rc_ctiv: 0,
        rc_temp_ctiv: 0,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    static MPSL_TIMESLOT_MEMORY: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::with_timeslots(
        mpsl_p,
        Irqs,
        lfclk_cfg,
        MPSL_TIMESLOT_MEMORY.init(mpsl::SessionMem::new()),
    )));
    spawner.spawn(unwrap!(mpsl_task(&*mpsl)));

    let hfclk = unwrap!(mpsl.request_hfclk().await);
    unwrap!(nrf_mpsl::Hfclk::wait().await);
    core::mem::forget(hfclk);

    let led = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    spawner.spawn(unwrap!(blink_task(led)));

    let flash_driver = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    static FLASH_MUTEX: StaticCell<Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>> =
        StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(Mutex::new(flash_driver));

    {
        let mut flash = flash_mutex.lock().await;
        let persistent = crate::flash::load_persistent_state(&mut *flash).await;
        let bonds = persistent.map(|(_, b)| b).unwrap_or_default();
        crate::ble::KEYBOARD_STATE.lock(|state| {
            let mut s = state.borrow_mut();
            s.active_slot = web::running_slot() as usize;
            s.bonds = bonds;
        });
        if option_env!("PAGER_SKIP_TRIAL_CONFIRM") != Some("1") {
            match crate::flash::confirm_running_slot(&mut *flash, web::running_slot()).await {
                Ok(true) => crate::log_msg!("BOOT:TRIAL_CONFIRMED:slot={}", web::running_slot()),
                Ok(false) => {}
                Err(error) => crate::log_msg!("BOOT:TRIAL_CONFIRM_ERROR:{:?}", error),
            }
        } else {
            crate::log_msg!("BOOT:TRIAL_CONFIRM_SKIPPED_FOR_TEST");
        }
    }

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
        p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    let mut rng = rng::Rng::new(p.RNG, Irqs);
    let mut sdc_mem = sdc::Mem::<4720>::new();
    let sdc = unwrap!(build_sdc(sdc_p, &mut rng, mpsl, &mut sdc_mem));

    let address = Address::random(ble_static_random_address());
    info!("Pager HID: our address = {:?}", address);

    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(sdc, &mut resources)
        .set_random_address(address)
        .build();
    sync_active_bond(&stack);
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Pager",
        appearance: &appearance::human_interface_device::GENERIC_HUMAN_INTERFACE_DEVICE,
    }))
    .unwrap();
    server.register_discovery_services();

    let mut adv_data = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids16(&[[0x12, 0x18]]),
            AdStructure::CompleteServiceUuids128(&[[
                0x8a, 0x12, 0xd7, 0xba, 0x46, 0x77, 0x30, 0xad, 0xe8, 0x46, 0x3e, 0x0b, 0x01, 0x00,
                0x7a, 0x9e,
            ]]),
        ],
        &mut adv_data[..],
    )
    .unwrap();
    let mut scan_data = [0u8; 31];
    let scan_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(b"Pager")],
        &mut scan_data[..],
    )
    .unwrap();

    unsafe {
        core::ptr::write_volatile(0x40027504 as *mut u32, 0);
    }
    Timer::after(Duration::from_millis(500)).await;
    unsafe {
        core::ptr::write_volatile(0x40027504 as *mut u32, 1);
    }

    static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    static USB_SERIAL: StaticCell<heapless::String<16>> = StaticCell::new();
    let vbus_detect: &'static SoftwareVbusDetect =
        &*VBUS_DETECT.init(SoftwareVbusDetect::new(true, true));
    vbus_detect.detected(true);
    vbus_detect.ready();
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    spawner.spawn(unwrap!(vbus_detect_task(vbus_detect)));

    let mut usb_config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT_NAME);
    usb_config.serial_number = Some(USB_SERIAL.init(factory_usb_serial()).as_str());
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;
    usb_config.device_class = 0xEF;
    usb_config.device_sub_class = 0x02;
    usb_config.device_protocol = 0x01;
    usb_config.composite_with_iads = true;

    static DEVICE_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONFIG_DESCRIPTOR: StaticCell<AlignedBuffer<768>> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONTROL_BUF: StaticCell<AlignedBuffer<128>> = StaticCell::new();

    let device_desc = &mut DEVICE_DESCRIPTOR
        .init(AlignedBuffer { data: [0; 256] })
        .data;
    let config_desc = &mut CONFIG_DESCRIPTOR
        .init(AlignedBuffer { data: [0; 768] })
        .data;
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
    info!("USB:BUILDER_READY");

    static ACM_STATE: StaticCell<AcmState> = StaticCell::new();
    let acm_class = CdcAcmClass::new(&mut builder, ACM_STATE.init(AcmState::new()), 64);

    static WEBUSB_CONTROL: StaticCell<webusb::LandingPageControl> = StaticCell::new();
    let webusb_transport = webusb::Transport::new(
        &mut builder,
        WEBUSB_CONTROL.init(webusb::LandingPageControl::new()),
        64,
    );
    info!("USB:WEBUSB_INTERFACE_READY");

    let usb = builder.build();
    info!("USB:DESCRIPTORS_BUILT");
    let (acm_sender, acm_receiver) = acm_class.split();

    spawner.spawn(unwrap!(usb_task(usb)));
    spawner.spawn(unwrap!(usb_logger_task(acm_sender)));
    spawner.spawn(unwrap!(usb_receiver_task(acm_receiver, flash_mutex)));
    spawner.spawn(unwrap!(webusb_task(webusb_transport)));
    spawner.spawn(unwrap!(web::ota_consumer_task(flash_mutex)));
    spawner.spawn(unwrap!(persist_keyboard_state_task(flash_mutex)));
    spawner.spawn(unwrap!(heartbeat_task()));

    info!("Pager HID & Web Server: starting advertising");
    let _ = embassy_futures::join::join(runner.run(), async {
        loop {
            crate::log_msg!("BLE:ADVERTISING");
            sync_active_bond(&stack);
            let advertiser = match peripheral
                .advertise(
                    &Default::default(),
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &adv_data[..len],
                        scan_data: &scan_data[..scan_len],
                    },
                )
                .await
            {
                Ok(advertiser) => advertiser,
                Err(error) => {
                    crate::log_msg!("BLE:ADVERTISE_ERROR:{:?}", error);
                    Timer::after(Duration::from_secs(1)).await;
                    continue;
                }
            };
            info!("Pager: waiting for connection...");
            let conn = match embassy_futures::select::select(
                advertiser.accept(),
                ble::BLE_COMMANDS.receive(),
            )
            .await
            {
                Either::First(Ok(conn)) => conn,
                Either::First(Err(error)) => {
                    crate::log_msg!("BLE:ACCEPT_ERROR:{:?}", error);
                    continue;
                }
                Either::Second(ble::BleCommand::SyncActiveBond) => {
                    sync_active_bond(&stack);
                    continue;
                }
                Either::Second(_) => continue,
            };
            let pairing_mode = ble::KEYBOARD_STATE.lock(|state| state.borrow().pairing_mode);
            let _ = conn.set_bondable(pairing_mode);
            let conn = match conn.with_attribute_server(&server) {
                Ok(conn) => conn,
                Err(error) => {
                    crate::log_msg!("BLE:GATT_SERVER_ERROR:{:?}", error);
                    continue;
                }
            };
            info!("Pager: connection established!");
            crate::log_msg!("BLE:CONNECTED");

            let mut status = 0u8;
            let mut persist_after_disconnect = false;
            loop {
                match embassy_futures::select::select3(
                    conn.next(),
                    ble::BLE_COMMANDS.receive(),
                    Timer::after(Duration::from_secs(1)),
                )
                .await
                {
                    Either3::First(event) => match event {
                        GattConnectionEvent::Disconnected { reason } => {
                            info!("Pager: disconnected {:?}", reason);
                            crate::log_msg!("BLE:DISCONNECTED:{:?}", reason);
                            break;
                        }
                        GattConnectionEvent::PairingComplete {
                            security_level,
                            bond,
                        } => {
                            info!(
                                "Pager: pairing complete! Level: {:?}, Bond: {:?}",
                                security_level, bond
                            );
                            crate::ble::KEYBOARD_STATE.lock(|state| {
                                let mut state = state.borrow_mut();
                                let active_slot = state.active_slot;
                                state.bonds[active_slot] = bond.clone();
                                state.pairing_mode = false;
                            });
                            persist_after_disconnect = true;
                        }
                        GattConnectionEvent::PairingFailed(err) => {
                            warn!("Pager: pairing failed: {:?}", err);
                        }
                        GattConnectionEvent::Gatt { event } => {
                            match &event {
                                GattEvent::Write(req) => {
                                    if let Ok(mode) = req.value(&server.custom_service.led) {
                                        if mode <= 2 {
                                            LED_MODE.signal(mode);
                                            info!("BLE LED mode set to {}", mode);
                                        } else {
                                            warn!("Ignoring invalid BLE LED mode {}", mode);
                                        }
                                    }
                                }
                                GattEvent::NotAllowed(req) => {
                                    crate::log_msg!("BLE:GATT_NOT_ALLOWED:handle={}", req.handle());
                                }
                                _ => {}
                            }
                            match event.accept() {
                                Ok(reply) => {
                                    reply.send().await;
                                }
                                Err(_) => crate::log_msg!("BLE:GATT_ACCEPT_ERROR"),
                            }
                        }
                        _ => {}
                    },
                    Either3::Second(command) => match command {
                        ble::BleCommand::SyncActiveBond => sync_active_bond(&stack),
                        ble::BleCommand::TypeString(text) => {
                            for ch in text.chars() {
                                let Some((modifier, keycode)) = ble::ascii_to_hid(ch) else {
                                    continue;
                                };
                                let report = [modifier, 0, keycode, 0, 0, 0, 0, 0];
                                if server
                                    .hid_service
                                    .input_keyboard
                                    .notify(&conn, &report, true)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                let _ = server
                                    .hid_service
                                    .boot_input_keyboard
                                    .notify(&conn, &report, true)
                                    .await;
                                Timer::after(Duration::from_millis(8)).await;
                                if server
                                    .hid_service
                                    .input_keyboard
                                    .notify(&conn, &[0; 8], true)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                let _ = server
                                    .hid_service
                                    .boot_input_keyboard
                                    .notify(&conn, &[0; 8], true)
                                    .await;
                                Timer::after(Duration::from_millis(8)).await;
                            }
                        }
                        ble::BleCommand::Disconnect => {
                            crate::log_msg!("BLE:COMMAND_DISCONNECT");
                            break;
                        }
                        ble::BleCommand::RestartAdvertising => {
                            crate::log_msg!("BLE:COMMAND_RESTART_ADVERTISING");
                            break;
                        }
                    },
                    Either3::Third(()) => {
                        status = status.wrapping_add(1);
                        let _ = server
                            .custom_service
                            .status
                            .notify(&conn, &status, false)
                            .await;
                    }
                }
            }
            if persist_after_disconnect {
                ble::PERSIST_STATE.signal(());
            }
        }
    })
    .await;
}
