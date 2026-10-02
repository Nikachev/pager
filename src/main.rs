#![no_std]
#![no_main]

#[cfg(any(
    all(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"),
    not(any(feature = "board-nice-nano-v2", feature = "board-xiao-nrf52840"))
))]
compile_error!("select exactly one Pager board feature");

mod ble;
mod faults;
mod flash;
mod led;
mod protocol;
mod runtime;
mod serial_task;
mod usb_detach;
mod webusb;
mod webusb_task;

pub use pager::{diagnostics, hid, layout};

pub use led::{blink_task, LED_MODE};
pub use serial_task::{usb_logger_task, usb_receiver_task};
pub use webusb_task::{webusb_reply, webusb_task};

use defmt::*;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::peripherals::RNG;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, pac, rng, usb};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State as AcmState};
use embassy_usb::{Builder, Config};
use nrf_sdc::mpsl;
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use panic_probe as _;
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    RNG => rng::InterruptHandler<RNG>;
    EGU0_SWI0 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    RTC0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    USBD => usb::InterruptHandler<embassy_nrf::peripherals::USBD>;
});

pub type NrfUsbDriver = Driver<'static, &'static SoftwareVbusDetect>;

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[repr(C, align(4))]
struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

pub use runtime::diagnostics::*;

pub const USB_VENDOR_ID: u16 = protocol::spec::APPLICATION_VID;
pub const USB_PRODUCT_ID: u16 = protocol::spec::APPLICATION_PID;
pub const USB_MANUFACTURER: &str = "Nikachev";
pub const USB_PRODUCT_NAME: &str = "Pager WebUSB+ACM";

#[embassy_executor::task]
async fn usb_task(mut usb: embassy_usb::UsbDevice<'static, NrfUsbDriver>) -> ! {
    usb.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = runtime::board::init();
    runtime::board::record_reset_reason();
    spawner.spawn(unwrap!(watchdog_task()));

    embassy_nrf::interrupt::USBD.set_priority(Priority::P2);

    let mpsl_p =
        mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = runtime::board::lfclk_config();
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

    #[cfg(feature = "board-nice-nano-v2")]
    let led = Output::new(p.P0_15, Level::High, OutputDrive::Standard);
    #[cfg(feature = "board-xiao-nrf52840")]
    let led = Output::new(p.P0_26, Level::High, OutputDrive::Standard);
    spawner.spawn(unwrap!(blink_task(led)));

    let flash_driver = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    static FLASH_MUTEX: StaticCell<Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>> =
        StaticCell::new();
    let flash_mutex = FLASH_MUTEX.init(Mutex::new(flash_driver));

    let storage_cursor = {
        let mut flash = flash_mutex.lock().await;
        let storage_deadline = embassy_time::Instant::now() + Duration::from_secs(2);
        let (cursor, persistent) = loop {
            match crate::flash::scan_storage(&mut *flash).await {
                Ok(storage) => break storage,
                Err(error) => {
                    crate::log_msg!("STORAGE:STARTUP_READ_ERROR:{:?}", error);
                    if embassy_time::Instant::now() >= storage_deadline {
                        crate::log_msg!("STORAGE:STARTUP_RECOVERY_DFU");
                        flash::enter_bootloader();
                    }
                    Timer::after(Duration::from_millis(250)).await;
                }
            }
        };
        crate::log_msg!("STORAGE:SCHEMA:7:RESTORED:{}", persistent.is_some());
        let duplicates_removed = crate::ble::KEYBOARD_STATE.lock(|state| {
            let mut s = state.borrow_mut();
            if let Some(persistent) = persistent {
                s.active_profile = persistent.active_profile;
                s.bluetooth_enabled = persistent.bluetooth_enabled;
                s.link_state = if persistent.bluetooth_enabled {
                    crate::ble::BleLinkState::Idle
                } else {
                    crate::ble::BleLinkState::BluetoothOff
                };
                s.bonds = persistent.bonds;
                s.cccd_flags = persistent.cccd_flags;
                s.slot_names = persistent.slot_names;
                s.device_name = persistent.device_name;
                // Pairing is deliberately not persistent. If reset interrupted
                // pairing for an empty active slot, recover to Bluetooth Off;
                // otherwise a failing radio path could trap USB in a reboot loop.
                if s.bluetooth_enabled
                    && s.active_profile.is_some_and(|slot| s.bonds[slot].is_none())
                {
                    s.bluetooth_enabled = false;
                    s.active_profile = None;
                    s.pairing_mode = false;
                    s.link_state = crate::ble::BleLinkState::BluetoothOff;
                }
                let duplicates_removed = ble::remove_duplicate_bonds(&mut s);
                if s.device_name.is_empty() {
                    let default_name = crate::ble::default_device_name();
                    let _ = s.device_name.push_str(default_name.as_str());
                }
                duplicates_removed
            } else {
                // Fresh boot and failed-first-commit rollback use exactly the
                // same canonical defaults, including the public device name.
                s.reduce(&ble::BleCommand::FactoryReset);
                false
            }
        });
        if duplicates_removed {
            crate::ble::request_persist();
        }
        cursor
    };

    pac::USBD
        .usbpullup()
        .write_value(pac::usbd::regs::Usbpullup(0));
    Timer::after(Duration::from_millis(500)).await;
    pac::USBD
        .usbpullup()
        .write_value(pac::usbd::regs::Usbpullup(1));

    static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    static USB_SERIAL: StaticCell<heapless::String<16>> = StaticCell::new();
    let vbus_detect: &'static SoftwareVbusDetect =
        &*VBUS_DETECT.init(SoftwareVbusDetect::new(true, true));
    vbus_detect.detected(true);
    vbus_detect.ready();
    let driver = Driver::new(p.USBD, Irqs, vbus_detect);

    let mut usb_config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    usb_config.manufacturer = Some(USB_MANUFACTURER);
    usb_config.product = Some(USB_PRODUCT_NAME);
    usb_config.serial_number = Some(
        USB_SERIAL
            .init(runtime::board::factory_usb_serial())
            .as_str(),
    );
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
    spawner.spawn(unwrap!(usb_receiver_task(acm_receiver)));
    spawner.spawn(unwrap!(webusb_task(webusb_transport)));
    spawner.spawn(unwrap!(runtime::persistence::persist_keyboard_state_task(
        flash_mutex,
        storage_cursor
    )));
    spawner.spawn(unwrap!(heartbeat_task()));

    runtime::ble_session::run(mpsl, p.RNG).await
}
