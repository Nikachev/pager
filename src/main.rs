#![no_std]
#![no_main]

mod ble;
mod flash;
mod web;

use ble::Server;
use core::cell::RefCell;
use core::sync::atomic::AtomicU32;
use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3};
use embassy_net::{Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4};
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
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State as AcmState};
use embassy_usb::class::cdc_ncm::embassy_net::{Device as NetDevice, State as NetState};
use embassy_usb::class::cdc_ncm::{CdcNcmClass, State};
use embassy_usb::{Builder, Config};
use embedded_io_async::Write;
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
                if let Either::Second(new_mode) = embassy_futures::select::select(
                    Timer::after(Duration::from_millis(50)),
                    LED_MODE.wait(),
                )
                .await
                {
                    mode = new_mode;
                    continue;
                }
                led.set_high(); // OFF
                if let Either::Second(new_mode) = embassy_futures::select::select(
                    Timer::after(Duration::from_millis(1950)),
                    LED_MODE.wait(),
                )
                .await
                {
                    mode = new_mode;
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

// nRF52840 WDT is intentionally configured directly: it must start before
// MPSL, USB or BLE initialization so a trial image that hangs during startup
// still resets into the bootloader. Once started Nordic's watchdog cannot be
// disabled without a reset.
const WDT_TASKS_START: *mut u32 = 0x4001_0000 as *mut u32;
const WDT_CRV: *mut u32 = 0x4001_0504 as *mut u32;
const WDT_RREN: *mut u32 = 0x4001_0508 as *mut u32;
const WDT_CONFIG: *mut u32 = 0x4001_050C as *mut u32;
const WDT_RR0: *mut u32 = 0x4001_0600 as *mut u32;
const WDT_RELOAD_MAGIC: u32 = 0x6E52_4635;
const WDT_RELOAD_TICKS: u32 = 10 * 32_768;

fn start_watchdog() {
    unsafe {
        // Run while the CPU sleeps, but pause when a debugger halts it.
        core::ptr::write_volatile(WDT_CONFIG, 0b01);
        core::ptr::write_volatile(WDT_CRV, WDT_RELOAD_TICKS);
        core::ptr::write_volatile(WDT_RREN, 1);
        core::ptr::write_volatile(WDT_TASKS_START, 1);
        core::ptr::write_volatile(WDT_RR0, WDT_RELOAD_MAGIC);
    }
}

#[embassy_executor::task]
async fn watchdog_task() -> ! {
    loop {
        Timer::after(Duration::from_secs(2)).await;
        if option_env!("PAGER_SKIP_WATCHDOG_FEED") != Some("1") {
            unsafe { core::ptr::write_volatile(WDT_RR0, WDT_RELOAD_MAGIC) };
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

pub const MTU: usize = 1514;
pub const IP_ADDRESS: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const GATEWAY: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const SUBNET_MASK: Ipv4Address = Ipv4Address::new(255, 255, 255, 0);
pub const DNS_SERVER: Ipv4Address = Ipv4Address::new(192, 168, 42, 1);
pub const DHCP_POOL_START: Ipv4Address = Ipv4Address::new(192, 168, 42, 10);
pub const DHCP_POOL_END: Ipv4Address = Ipv4Address::new(192, 168, 42, 50);

pub const USB_VENDOR_ID: u16 = 0x1209;
pub const USB_PRODUCT_ID: u16 = 0x0001;
pub const USB_MANUFACTURER: &str = "Nikachev";
pub const USB_PRODUCT_NAME: &str = "Pager NCM+ACM";
const FICR_DEVICEID0: *const u32 = 0x1000_0060 as *const u32;
const FICR_DEVICEID1: *const u32 = 0x1000_0064 as *const u32;

fn factory_usb_serial() -> heapless::String<16> {
    // DEVICEID is programmed by Nordic at manufacture and is readable on
    // nRF52840 without enabling peripheral clocks.
    let low = unsafe { core::ptr::read_volatile(FICR_DEVICEID0) };
    let high = unsafe { core::ptr::read_volatile(FICR_DEVICEID1) };
    let mut serial = heapless::String::new();
    let _ = core::fmt::write(&mut serial, format_args!("{:08x}{:08x}", high, low));
    serial
}

pub const HOST_MAC_ADDR: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
pub const DEVICE_MAC_ADDR: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];

/// Build a stable Bluetooth static-random identity from the nRF52 factory ID.
///
/// The FICR device ID is programmed once by Nordic.  Keeping the address
/// stable is essential for bonded hosts and means each board gets a different
/// identity without a manufacturing-time configuration file.  `Address` uses
/// HCI stores the address least-significant byte first, therefore bits 47:46
/// belong to the final byte and must be `11` for a static-random address.
fn ble_static_random_address() -> [u8; 6] {
    const FICR_DEVICEID0: *const u32 = 0x1000_0060 as *const u32;
    const FICR_DEVICEID1: *const u32 = 0x1000_0064 as *const u32;

    let id0 = unsafe { core::ptr::read_volatile(FICR_DEVICEID0) };
    let id1 = unsafe { core::ptr::read_volatile(FICR_DEVICEID1) };
    let mut address = [0u8; 6];
    address[..4].copy_from_slice(&id0.to_be_bytes());
    address[4..].copy_from_slice(&id1.to_be_bytes()[2..]);
    address[5] = (address[5] & 0x3f) | 0xc0;
    address
}

type MyDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[repr(C, align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

pub static LOG_CHANNEL: Channel<ThreadModeRawMutex, heapless::String<128>, 32> = Channel::new();
pub static DROPPED_LOGS: AtomicU32 = AtomicU32::new(0);
pub static LOG_HISTORY: SyncMutex<
    ThreadModeRawMutex,
    RefCell<heapless::Deque<heapless::String<128>, 32>>,
> = SyncMutex::new(RefCell::new(heapless::Deque::new()));

pub fn get_logs() -> heapless::Vec<heapless::String<128>, 32> {
    LOG_HISTORY.lock(|hist| {
        let mut v = heapless::Vec::new();
        for item in hist.borrow().iter() {
            let _ = v.push(item.clone());
        }
        v
    })
}

pub static LED_MODE: embassy_sync::signal::Signal<ThreadModeRawMutex, u8> =
    embassy_sync::signal::Signal::new();

#[macro_export]
macro_rules! log_msg {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        let mut s = heapless::String::<128>::new();
        let _ = core::fmt::write(&mut s, format_args!($($arg)*));
        if $crate::LOG_CHANNEL.try_send(s.clone()).is_err() {
            $crate::DROPPED_LOGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        $crate::LOG_HISTORY.lock(|hist| {
            let mut h = hist.borrow_mut();
            if h.is_full() {
                h.pop_front();
            }
            let _ = h.push_back(s);
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
async fn usb_ncm_task(
    runner: embassy_usb::class::cdc_ncm::embassy_net::Runner<'static, MyDriver, MTU>,
) -> ! {
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
    let _ = sender.write_all(b"Pager serial logger started.\r\n").await;
    loop {
        let msg = LOG_CHANNEL.receive().await;
        if sender.write_all(msg.as_bytes()).await.is_err() {
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }
        let _ = sender.write_all(b"\r\n").await;
    }
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
        // NVMC operations are not cancellation-safe: aborting an erase or
        // program future halfway through can leave the flash/MPSL subsystem in
        // an undefined state and take USB-NCM down with it. Let this bounded
        // one-page write finish, then report a real driver error.
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
                            let cmd_str = core::str::from_utf8(&cmd_buf[..cmd_len])
                                .unwrap_or("")
                                .trim();
                            cmd_len = 0;

                            if cmd_str == "dfu" || cmd_str == "reboot" {
                                log_msg!("Rebooting device...");
                                Timer::after(Duration::from_millis(100)).await;
                                cortex_m::peripheral::SCB::sys_reset();
                            } else if let Some(stripped) = cmd_str.strip_prefix("update") {
                                let mut fields = stripped.split_whitespace();
                                let content_len: usize =
                                    match fields.next().and_then(|s| s.parse().ok()) {
                                        Some(l) if l > 0 && l <= web::MAX_BIN_SIZE => l,
                                        _ => {
                                            log_msg!("SERIAL_UPDATE:ERROR_INVALID_SIZE");
                                            continue;
                                        }
                                    };
                                let expected_crc = match fields.next().and_then(|s| {
                                    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
                                }) {
                                    Some(crc) if fields.next().is_none() => crc,
                                    _ => {
                                        log_msg!("SERIAL_UPDATE:ERROR_INVALID_CHECKSUM");
                                        continue;
                                    }
                                };
                                if !crate::flash::try_start_flashing() {
                                    log_msg!("SERIAL_UPDATE:REJECTED_DFU_IN_PROGRESS");
                                    continue;
                                }
                                log_msg!("SERIAL_UPDATE:START:{}", content_len);
                                log_msg!(
                                    "SERIAL_UPDATE:READY:{}:{:08x}",
                                    content_len,
                                    expected_crc
                                );
                                let mut flash = flash_mutex.lock().await;
                                log_msg!("SERIAL_DFU_MUTEX_LOCKED");
                                let mut writer = crate::flash::OtaWriter::new(
                                    &mut *flash,
                                    web::inactive_slot_manifest_addr(),
                                    web::inactive_slot_manifest_addr() + web::MAX_BIN_SIZE as u32,
                                );

                                let mut total_read = 0;
                                let mut crc = crate::flash::CRC32_INIT;
                                let mut write_failed = false;
                                let mut chunk_buf = [0u8; 64];
                                let mut idle_count = 0;
                                while total_read < content_len {
                                    match receiver.read_packet(&mut chunk_buf).await {
                                        Ok(cn) if cn > 0 => {
                                            idle_count = 0;
                                            let to_write =
                                                core::cmp::min(cn, content_len - total_read);
                                            if let Err(e) =
                                                writer.write_chunk(&chunk_buf[..to_write]).await
                                            {
                                                log_msg!("SERIAL_UPDATE:FLASH_ERROR:{:?}", e);
                                                write_failed = true;
                                                break;
                                            }
                                            crc = crate::flash::crc32_update(
                                                crc,
                                                &chunk_buf[..to_write],
                                            );
                                            total_read += to_write;
                                        }
                                        _ => {
                                            idle_count += 1;
                                            if idle_count > 1000 {
                                                log_msg!("SERIAL_UPDATE:TIMEOUT");
                                                break;
                                            }
                                            Timer::after(Duration::from_millis(1)).await;
                                        }
                                    }
                                }
                                if !write_failed
                                    && total_read == content_len
                                    && crate::flash::crc32_finalize(crc) == expected_crc
                                {
                                    if let Err(e) = writer.flush().await {
                                        log_msg!("SERIAL_UPDATE:FLASH_ERROR:{:?}", e);
                                        crate::flash::stop_flashing();
                                        continue;
                                    }
                                    let _ = writer;
                                    match crate::flash::package_targets_slot(
                                        &mut *flash,
                                        web::inactive_slot_manifest_addr(),
                                        content_len,
                                        web::inactive_slot(),
                                    )
                                    .await
                                    {
                                        Ok(true) => {}
                                        Ok(false) => {
                                            log_msg!("SERIAL_UPDATE:ERROR_TARGET_SLOT");
                                            crate::flash::stop_flashing();
                                            continue;
                                        }
                                        Err(e) => {
                                            log_msg!("SERIAL_UPDATE:FLASH_ERROR:{:?}", e);
                                            crate::flash::stop_flashing();
                                            continue;
                                        }
                                    }
                                    log_msg!("SERIAL_UPDATE:COMPLETE");
                                    Timer::after(Duration::from_millis(500)).await;
                                    crate::flash::stop_flashing();
                                    web::reset_after_usb_detach().await;
                                } else {
                                    if !write_failed && total_read == content_len {
                                        log_msg!("SERIAL_UPDATE:CHECKSUM_MISMATCH");
                                    }
                                    crate::flash::stop_flashing();
                                }
                            } else if let Some(text) = cmd_str.strip_prefix("type ") {
                                log_msg!("Serial command type: {}", text);
                                let mut s = heapless::String::<128>::new();
                                let _ = s.push_str(text);
                                if !ble::try_send_command(ble::BleCommand::TypeString(s)) {
                                    log_msg!("BLE command queue full during serial type request");
                                }
                            } else if cmd_str == "pair" {
                                log_msg!("Serial command: Triggering pairing mode");
                                if !ble::try_send_command(ble::BleCommand::RestartAdvertising) {
                                    log_msg!(
                                        "BLE command queue full during serial pairing request"
                                    );
                                }
                            } else if cmd_str == "ping" {
                                // Keep a side-effect-free CDC command for diagnostics and HIL smoke tests.
                                log_msg!("SERIAL:PONG");
                            } else if cmd_str == "clear_bonds" {
                                log_msg!("Serial command: Clearing persistent bonds");
                                ble::KEYBOARD_STATE.lock(|state| {
                                    let mut state = state.borrow_mut();
                                    state.bonds = [None, None, None];
                                });
                                ble::PERSIST_STATE.signal(());
                                let _ = ble::try_send_command(ble::BleCommand::SyncActiveBond);
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

    start_watchdog();
    spawner.spawn(unwrap!(watchdog_task()));

    embassy_nrf::interrupt::USBD.set_priority(Priority::P2);

    // --- MPSL & SDC ---
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
    // `nrf_mpsl::Flash` schedules each erase/write in an MPSL timeslot.  A
    // plain `new` MPSL instance has no session storage, so every flash request
    // fails with ENOMEM and an OTA upload eventually fills the TCP window.
    static MPSL_TIMESLOT_MEMORY: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::with_timeslots(
        mpsl_p,
        Irqs,
        lfclk_cfg,
        MPSL_TIMESLOT_MEMORY.init(mpsl::SessionMem::new()),
    )));
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
    static FLASH_MUTEX: StaticCell<Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>> =
        StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(Mutex::new(flash_driver));

    // Restore persistent keyboard state from flash storage
    {
        let mut flash = flash_mutex.lock().await;
        if let Some((active_slot, bonds)) = crate::flash::load_persistent_state(&mut *flash).await {
            crate::log_msg!(
                "Restored persistent keyboard state from flash: active_slot={}",
                active_slot
            );
            crate::ble::KEYBOARD_STATE.lock(|state| {
                let mut s = state.borrow_mut();
                s.active_slot = active_slot;
                s.bonds = bonds;
            });
        }
    }

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
        p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    let mut rng = rng::Rng::new(p.RNG, Irqs);
    // Generate network seed before passing rng to SDC (which holds &mut rng)
    let mut seed_buf = [0u8; 8];
    rng.blocking_fill_bytes(&mut seed_buf);
    let net_seed = u64::from_le_bytes(seed_buf);
    let mut sdc_mem = sdc::Mem::<4720>::new();
    let sdc = unwrap!(build_sdc(sdc_p, &mut rng, mpsl, &mut sdc_mem));

    // --- trouble host stack ---
    let address = Address::random(ble_static_random_address());
    info!("Pager HID: our address = {:?}", address);

    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(sdc, &mut resources)
        .set_random_address(address)
        .build();
    // `trouble-host` owns the live security database. Rehydrate it before
    // advertising so a previously bonded host can request encryption on its
    // first reconnect after reset.
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
            // Bluetooth advertising UUIDs are encoded little-endian.
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

    // Force USB D+ pullup low then high to ensure host sees clean USB disconnect/reconnect
    unsafe {
        core::ptr::write_volatile(0x40027504 as *mut u32, 0);
    }
    Timer::after(Duration::from_millis(500)).await;
    unsafe {
        core::ptr::write_volatile(0x40027504 as *mut u32, 1);
    }

    // --- USB Initialization ---
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
    static CONFIG_DESCRIPTOR: StaticCell<AlignedBuffer<512>> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<AlignedBuffer<256>> = StaticCell::new();
    static CONTROL_BUF: StaticCell<AlignedBuffer<128>> = StaticCell::new();

    let device_desc = &mut DEVICE_DESCRIPTOR
        .init(AlignedBuffer { data: [0; 256] })
        .data;
    let config_desc = &mut CONFIG_DESCRIPTOR
        .init(AlignedBuffer { data: [0; 512] })
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
    let (net_runner, device) = class
        .into_embassy_net_device::<MTU, 4, 4>(NET_STATE.init(NetState::new()), DEVICE_MAC_ADDR);

    spawner.spawn(unwrap!(usb_ncm_task(net_runner)));

    let net_config = StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 42, 1), 24),
        gateway: Some(Ipv4Address::new(192, 168, 42, 1)),
        dns_servers: heapless::Vec::<Ipv4Address, 3>::new(),
    };

    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

    let (stack_net, runner_net) = embassy_net::new(
        device,
        embassy_net::Config::ipv4_static(net_config),
        RESOURCES.init(StackResources::new()),
        net_seed,
    );

    spawner.spawn(unwrap!(net_task(runner_net)));
    spawner.spawn(unwrap!(dhcp_task(stack_net)));
    spawner.spawn(unwrap!(web::ota_consumer_task(flash_mutex)));
    spawner.spawn(unwrap!(web::web_task(stack_net)));
    spawner.spawn(unwrap!(persist_keyboard_state_task(flash_mutex)));
    spawner.spawn(unwrap!(heartbeat_task()));

    // All essential subsystems (radio, USB, networking and persistence) are
    // initialized. Confirm an A/B trial only now; a reset before this point
    // leaves the previous image selected by the bootloader.
    if option_env!("PAGER_SKIP_TRIAL_CONFIRM") != Some("1") {
        let mut flash = flash_mutex.lock().await;
        match crate::flash::confirm_running_slot(&mut *flash, web::running_slot()).await {
            Ok(true) => crate::log_msg!("BOOT:TRIAL_CONFIRMED:slot={}", web::running_slot()),
            Ok(false) => {}
            Err(error) => crate::log_msg!("BOOT:TRIAL_CONFIRM_ERROR:{:?}", error),
        }
    } else {
        // HIL-only rollback fixture. The Makefile never enables it unless the
        // developer explicitly sets TRIAL_NO_CONFIRM=1.
        crate::log_msg!("BOOT:TRIAL_CONFIRM_SKIPPED_FOR_TEST");
    }

    info!("Pager HID & Web Server: starting advertising");
    let _ = embassy_futures::join::join(runner.run(), async {
        loop {
            crate::log_msg!("BLE:ADVERTISING");
            // Refresh the live security database only between connections.
            // Updating it while an encrypted link is completing pairing can
            // invalidate the central's in-flight LTK transaction.
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
            // Configure security before starting the attribute server, as the
            // first service-discovery request may arrive immediately after
            // the link is accepted.
            // Existing bonds can reconnect at any time, but new pairing is
            // accepted only after the physical-USB control plane explicitly
            // enabled pairing mode for the active profile.
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
                            // NVMC persistence must not overlap an active BLE
                            // security procedure. Save after this central is
                            // disconnected and the connection object is dropped.
                            persist_after_disconnect = true;
                        }
                        GattConnectionEvent::PairingFailed(err) => {
                            warn!("Pager: pairing failed: {:?}", err);
                        }
                        GattConnectionEvent::Gatt { event } => {
                            // Every ATT event must be accepted and its reply
                            // sent explicitly. Dropping an `Other` event only
                            // constructs a reply; CoreBluetooth then waits for
                            // it and terminates the connection.
                            match &event {
                                GattEvent::Write(req) => {
                                    // The custom LED control is a `u8`: 0 = auto
                                    // blink, 1 = off, 2 = on.  Accepting a GATT
                                    // write alone only acknowledges it; applying
                                    // the value is what wakes the LED task.
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
                            // Dropping the GATT connection restarts advertising with the
                            // latest slot/pairing configuration.
                            break;
                        }
                    },
                    Either3::Third(()) => {
                        // A lightweight liveness signal for BLE clients.  It
                        // is intentionally independent from the USB log
                        // heartbeat: `notify` simply does nothing until the
                        // client enables this characteristic's CCCD.
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
