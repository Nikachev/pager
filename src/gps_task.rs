//! XIAO L76K UART diagnostics, isolated from USB and BLE.
#[cfg(feature = "board-xiao-nrf52840")]
use core::cell::RefCell;
#[cfg(feature = "board-xiao-nrf52840")]
use embassy_sync::blocking_mutex::{raw::ThreadModeRawMutex, Mutex};
#[cfg(feature = "board-xiao-nrf52840")]
use embassy_time::Instant;

#[cfg(feature = "board-xiao-nrf52840")]
struct State {
    receiver: pager::gps::Receiver,
    startup: pager::gps::Startup,
    last_nmea: Option<Instant>,
    last_gga: Option<Instant>,
    uart_errors: u32,
    sentences: heapless::Vec<(Instant, heapless::String<128>), 24>,
    last_fix: Option<((i32, i32), Instant, Option<u64>)>,
}
#[cfg(feature = "board-xiao-nrf52840")]
static STATE: Mutex<ThreadModeRawMutex, RefCell<Option<State>>> = Mutex::new(RefCell::new(None));

/// Latest checksum-valid NMEA packets, newest first. Explicit diagnostic access only.
pub fn sentence(index: u8) -> heapless::String<192> {
    let mut result = heapless::String::new();
    #[cfg(feature = "board-nice-nano-v2")]
    {
        let _ = index;
        let _ = result.push_str("supported=0");
    }
    #[cfg(feature = "board-xiao-nrf52840")]
    STATE.lock(|cell| {
        use core::fmt::Write;
        let state = cell.borrow();
        if let Some(s) = state.as_ref() {
            if let Some((t, line)) = s.sentences.iter().rev().nth(index as usize) {
                let _ = write!(
                    result,
                    "age_ms={};nmea={}",
                    Instant::now().duration_since(*t).as_millis(),
                    line
                );
            }
        }
    });
    result
}

pub fn status() -> heapless::String<512> {
    #[cfg(feature = "board-xiao-nrf52840")]
    use core::fmt::Write;
    let mut result = heapless::String::new();
    #[cfg(feature = "board-nice-nano-v2")]
    {
        let _ = result.push_str("supported=0");
    }
    #[cfg(feature = "board-xiao-nrf52840")]
    STATE.lock(|cell| {
        let state = cell.borrow();
        if let Some(s) = state.as_ref() {
            let age = |t: Option<Instant>| t.map(|t| Instant::now().duration_since(t).as_millis());
            let fresh = |t| age(t).is_some_and(|ms| ms < 5000);
            let position = if fresh(s.last_gga) { s.receiver.position } else { None };
            let utc = crate::runtime::clock::now_utc_ms();
            let sync_age = crate::runtime::clock::sync_age_ms();
            let source = if utc.is_none() { "unsynced" } else if sync_age.is_some_and(|a| a < 5000) { "gps" } else { "holdover" };
            let _ = write!(result, "supported=1;connected={};fix={};bytes={};valid={};invalid={};uart_errors={};quality={};satellites={};nmea_age_ms={};gga_age_ms={};latitude_e6={};longitude_e6={};utc_ms={};time_source={};time_age_ms={}",
                fresh(s.last_nmea) as u8, position.is_some() as u8,
                s.receiver.bytes, s.receiver.valid, s.receiver.invalid, s.uart_errors,
                s.receiver.quality, s.receiver.satellites, age(s.last_nmea).unwrap_or(u64::MAX),
                age(s.last_gga).unwrap_or(u64::MAX),
                position.map(|p| i64::from(p.0)).unwrap_or(i64::MAX),
                position.map(|p| i64::from(p.1)).unwrap_or(i64::MAX),
                utc.unwrap_or(0), source, sync_age.unwrap_or(u64::MAX));
            if let Some((position, captured, utc)) = s.last_fix {
                let _ = write!(result, ";last_latitude_e6={};last_longitude_e6={};last_fix_utc_ms={};last_fix_age_ms={}",
                    position.0, position.1, utc.unwrap_or(0), Instant::now().duration_since(captured).as_millis());
            }
            // Raw GGA remains optional for older diagnostic clients, after the structured fields.
            if result.len() + 5 + s.receiver.gga.len() <= result.capacity() {
                let _ = write!(result, ";gga={}", s.receiver.gga);
            }
        } else { let _ = result.push_str("supported=1;connected=0;fix=0"); }
    });
    result
}

#[cfg(feature = "board-xiao-nrf52840")]
#[embassy_executor::task]
pub async fn gps_task(mut uart: embassy_nrf::buffered_uarte::BufferedUarte<'static>) -> ! {
    STATE.lock(|s| {
        *s.borrow_mut() = Some(State {
            receiver: Default::default(),
            startup: Default::default(),
            last_nmea: None,
            last_gga: None,
            uart_errors: 0,
            last_fix: None,
            sentences: heapless::Vec::new(),
        })
    });
    let mut buffer = [0; 128];
    loop {
        match uart.read(&mut buffer).await {
            Ok(n) => STATE.lock(|cell| {
                let mut state = cell.borrow_mut();
                let s = state.as_mut().unwrap();
                for &b in &buffer[..n] {
                    let accepted = s.receiver.feed(b);
                    s.startup
                        .observe(Instant::now().as_millis(), &s.receiver, accepted);
                    if accepted {
                        s.last_nmea = Some(Instant::now());
                        if s.sentences.is_full() {
                            s.sentences.remove(0);
                        }
                        let _ = s
                            .sentences
                            .push((Instant::now(), s.receiver.last_sentence.clone()));
                        if let Some(utc) = s.receiver.utc_update_ms {
                            crate::runtime::clock::correct_from_gps(utc);
                        }
                        // GGA may repeat byte-for-byte; recognize sentence type on every update.
                        if s.receiver.gga_updated {
                            s.last_gga = s.last_nmea;
                            if let Some(position) = s.receiver.position {
                                s.last_fix =
                                    Some((position, Instant::now(), crate::clock::now_utc_ms()));
                            }
                        }
                    }
                }
            }),
            Err(_) => {
                STATE.lock(|cell| {
                    let mut s = cell.borrow_mut();
                    let s = s.as_mut().unwrap();
                    s.uart_errors = s.uart_errors.saturating_add(1);
                });
                embassy_time::Timer::after_millis(10).await;
            }
        }
    }
}

/// Missing events are empty; timestamps use embassy monotonic boot origin.
pub fn startup() -> heapless::String<512> {
    let mut result = heapless::String::new();
    #[cfg(feature = "board-nice-nano-v2")]
    let _ = result.push_str("supported=0");
    #[cfg(feature = "board-xiao-nrf52840")]
    STATE.lock(|cell| {
        use core::fmt::Write;
        let _ = write!(
            result,
            "supported=1;uptime_ms={};baseline=firmware_monotonic",
            Instant::now().as_millis()
        );
        if let Some(s) = cell.borrow().as_ref() {
            for (key, value) in [
                ("first_byte_ms", s.startup.byte),
                ("first_nmea_ms", s.startup.nmea),
                ("first_view_ms", s.startup.view),
                ("first_signal_ms", s.startup.signal),
                ("first_fix_ms", s.startup.fix),
                ("first_utc_ms", s.startup.utc),
            ] {
                let _ = write!(result, ";{}=", key);
                if let Some(value) = value {
                    let _ = write!(result, "{}", value);
                }
            }
        }
    });
    result
}
