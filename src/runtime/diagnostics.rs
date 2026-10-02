//! Runtime logging and subsystem liveness; counters/codecs stay host-testable.
use crate::diagnostics;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_futures::select::Either;
use embassy_nrf::pac;
use embassy_sync::blocking_mutex::{raw::ThreadModeRawMutex, Mutex as SyncMutex};
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
pub static LOG_CHANNEL: Channel<ThreadModeRawMutex, heapless::String<128>, 32> = Channel::new();
pub static HISTORY_EVICTIONS: AtomicU32 = AtomicU32::new(0);
pub static LOG_HIGH_WATER: AtomicU32 = AtomicU32::new(0);
pub static STORAGE_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static RESET_REASON: AtomicU32 = AtomicU32::new(0);
pub static DROPPED_LOGS: AtomicU32 = AtomicU32::new(0);

// Opt-in progress tracing carries only transport counts and public command IDs,
// never request contents. It remains observable over CDC if bulk USB stalls.
static USB_PHASE: AtomicU32 = AtomicU32::new(0);
static USB_RX_PACKETS: AtomicU32 = AtomicU32::new(0);
static USB_RX_BYTES: AtomicU32 = AtomicU32::new(0);
static USB_REQUEST: AtomicU32 = AtomicU32::new(0);
static USB_OPCODE: AtomicU32 = AtomicU32::new(0);

pub fn trace_usb_phase(phase: u32) {
    if option_env!("PAGER_USB_TRACE") == Some("1") {
        USB_PHASE.store(phase, Ordering::Relaxed);
    }
}

pub fn trace_usb_receive(bytes: usize) {
    if option_env!("PAGER_USB_TRACE") == Some("1") {
        diagnostics::increment_saturated(&USB_RX_PACKETS);
        USB_RX_BYTES.fetch_add(bytes as u32, Ordering::Relaxed);
    }
}

pub fn trace_usb_command(request: u32, opcode: u8) {
    if option_env!("PAGER_USB_TRACE") == Some("1") {
        USB_REQUEST.store(request, Ordering::Relaxed);
        USB_OPCODE.store(opcode as u32, Ordering::Relaxed);
    }
}

type LogHistory = RefCell<heapless::Vec<heapless::String<128>, 32>>;
static LOG_HISTORY: SyncMutex<ThreadModeRawMutex, LogHistory> =
    SyncMutex::new(RefCell::new(heapless::Vec::new()));

pub fn with_logs<F: FnMut(&str)>(mut f: F) {
    LOG_HISTORY.lock(|hist| {
        for item in hist.borrow().iter().rev() {
            f(item.as_str());
        }
    });
}

pub fn record_log(line: heapless::String<128>) {
    LOG_HISTORY.lock(|history| {
        let mut history = history.borrow_mut();
        if history.is_full() {
            history.remove(0);
            diagnostics::increment_saturated(&HISTORY_EVICTIONS);
        }
        let _ = history.push(line);
        LOG_HIGH_WATER.fetch_max(history.len() as u32, Ordering::Relaxed);
    });
}

#[macro_export]
macro_rules! log_msg {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        let mut s = heapless::String::<128>::new();
        let _ = core::fmt::write(&mut s, format_args!($($arg)*));
        $crate::record_log(s.clone());
        if $crate::LOG_CHANNEL.try_send(s.clone()).is_err() {
            let _ = $crate::LOG_CHANNEL.try_receive();
            let _ = $crate::LOG_CHANNEL.try_send(s);
            $crate::diagnostics::increment_saturated(&$crate::DROPPED_LOGS);
        }
    }};
}

const WDT_RELOAD_MAGIC: u32 = 0x6E52_4635;
const WDT_RELOAD_TICKS: u32 = 10 * 32_768;

pub fn start_watchdog() {
    pac::WDT.config().write_value(pac::wdt::regs::Config(0b01));
    pac::WDT.crv().write_value(WDT_RELOAD_TICKS);
    pac::WDT.rren().write_value(pac::wdt::regs::Rren(1));
    pac::WDT.tasks_start().write_value(1);
    pac::WDT
        .rr(0)
        .write_value(pac::wdt::regs::Rr(WDT_RELOAD_MAGIC));
}

pub const HEARTBEAT_BLINK: u8 = 1 << 0;
pub const HEARTBEAT_WEBUSB: u8 = 1 << 1;
pub const HEARTBEAT_BLE: u8 = 1 << 2;
pub const HEARTBEAT_STORAGE: u8 = 1 << 3;
pub static REQUIRED_HEARTBEATS: AtomicU32 = AtomicU32::new(1);
pub static TASK_HEARTBEATS: AtomicU32 = AtomicU32::new(0);

pub fn signal_heartbeat(flag: u8) {
    TASK_HEARTBEATS.fetch_or(flag as u32, Ordering::Relaxed);
}

pub async fn idle_progress<F: core::future::Future>(flag: u8, future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    loop {
        signal_heartbeat(flag);
        match embassy_futures::select::select(future.as_mut(), Timer::after(Duration::from_secs(1)))
            .await
        {
            Either::First(value) => return value,
            Either::Second(()) => {}
        }
    }
}

#[embassy_executor::task]
pub async fn watchdog_task() -> ! {
    let mut progress = pager::watchdog::ProgressWatchdog::default();
    loop {
        Timer::after(Duration::from_secs(2)).await;
        let mask = TASK_HEARTBEATS.swap(0, Ordering::Relaxed);
        if progress.observe(REQUIRED_HEARTBEATS.load(Ordering::Relaxed), mask) {
            if option_env!("PAGER_SKIP_WATCHDOG_FEED") != Some("1") {
                pac::WDT
                    .rr(0)
                    .write_value(pac::wdt::regs::Rr(WDT_RELOAD_MAGIC));
            }
        } else {
            defmt::warn!("Watchdog: missing task heartbeat mask {:x}", mask);
        }
    }
}

#[embassy_executor::task]
pub async fn heartbeat_task() -> ! {
    let mut count = 0;
    loop {
        Timer::after(Duration::from_secs(10)).await;
        count += 10;
        crate::log_msg!("System heartbeat uptime: {}s", count);
        if option_env!("PAGER_USB_TRACE") == Some("1") {
            crate::log_msg!(
                "USB:PROGRESS:phase={}:packets={}:bytes={}:request={}:opcode={}",
                USB_PHASE.load(Ordering::Relaxed),
                USB_RX_PACKETS.load(Ordering::Relaxed),
                USB_RX_BYTES.load(Ordering::Relaxed),
                USB_REQUEST.load(Ordering::Relaxed),
                USB_OPCODE.load(Ordering::Relaxed)
            );
        }
    }
}
