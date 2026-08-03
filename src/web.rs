use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::pipe::Pipe;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};

pub static OTA_PIPE: Pipe<ThreadModeRawMutex, 16384> = Pipe::new();
pub static OTA_COMMAND_SIGNAL: Signal<ThreadModeRawMutex, OtaCommand> = Signal::new();
pub static OTA_CANCEL_SIGNAL: Signal<ThreadModeRawMutex, ()> = Signal::new();
pub static OTA_READY_SIGNAL: Signal<ThreadModeRawMutex, Result<(), ()>> = Signal::new();
pub static OTA_RESULT_SIGNAL: Signal<ThreadModeRawMutex, Result<(), ()>> = Signal::new();

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum OtaCommand {
    Start {
        content_len: usize,
        expected_crc: u32,
        target_start: u32,
        target_slot: u32,
    },
    Cancel,
}

#[embassy_executor::task]
pub async fn ota_consumer_task(
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>,
) -> ! {
    loop {
        let cmd = OTA_COMMAND_SIGNAL.wait().await;
        let (content_len, expected_crc, target_start, target_slot) = match cmd {
            OtaCommand::Start {
                content_len,
                expected_crc,
                target_start,
                target_slot,
            } => (content_len, expected_crc, target_start, target_slot),
            OtaCommand::Cancel => {
                OTA_PIPE.clear();
                continue;
            }
        };

        crate::log_msg!("OTA Consumer started for {} bytes", content_len);
        OTA_PIPE.clear();
        OTA_CANCEL_SIGNAL.reset();

        let mut flash = flash_mutex.lock().await;

        crate::log_msg!(
            "OTA Consumer ready for streaming {} bytes (on-the-fly page erasing)",
            content_len
        );
        OTA_READY_SIGNAL.signal(Ok(()));

        let mut writer = crate::flash::OtaWriter::new(
            &mut *flash,
            target_start,
            target_start + MAX_BIN_SIZE as u32,
        );
        let mut total_written = 0;
        let mut crc = crate::flash::CRC32_INIT;
        let mut write_err = false;
        let mut read_buf = [0u8; 1024];

        while total_written < content_len && !write_err {
            let to_read = core::cmp::min(read_buf.len(), content_len - total_written);
            let mut pipe = &OTA_PIPE;
            let read_fut = embedded_io_async::Read::read(&mut pipe, &mut read_buf[..to_read]);
            let cancel_fut = OTA_CANCEL_SIGNAL.wait();

            let n = match embassy_futures::select::select(read_fut, cancel_fut).await {
                embassy_futures::select::Either::First(Ok(n)) if n > 0 => n,
                embassy_futures::select::Either::First(_) => {
                    write_err = true;
                    break;
                }
                embassy_futures::select::Either::Second(_) => {
                    crate::log_msg!("OTA Consumer: received cancellation signal");
                    write_err = true;
                    break;
                }
            };

            if let Err(e) = writer.write_chunk(&read_buf[..n]).await {
                crate::log_msg!("OTA Consumer write error: {:?}", e);
                write_err = true;
                break;
            }
            crc = crate::flash::crc32_update(crc, &read_buf[..n]);
            total_written += n;
            crate::signal_heartbeat(crate::HEARTBEAT_BLINK);
        }

        let mut ready_to_validate = false;
        if !write_err
            && total_written == content_len
            && crate::flash::crc32_finalize(crc) == expected_crc
        {
            if let Err(e) = writer.flush().await {
                crate::log_msg!("OTA Consumer flush error: {:?}", e);
                OTA_RESULT_SIGNAL.signal(Err(()));
            } else {
                ready_to_validate = true;
            }
        } else {
            crate::log_msg!(
                "OTA Consumer write rejected: {} / {}, checksum={:08x}",
                total_written,
                content_len,
                crate::flash::crc32_finalize(crc)
            );
            OTA_RESULT_SIGNAL.signal(Err(()));
        }

        let mut completed = false;
        if ready_to_validate {
            match crate::flash::package_targets_slot(
                &mut *flash,
                target_start,
                content_len,
                target_slot,
            )
            .await
            {
                Ok(true) => {
                    crate::log_msg!(
                        "OTA Consumer successfully finished writing {} bytes",
                        total_written
                    );
                    OTA_RESULT_SIGNAL.signal(Ok(()));
                    completed = true;
                }
                Ok(false) => {
                    crate::log_msg!("OTA package target slot mismatch");
                    OTA_RESULT_SIGNAL.signal(Err(()));
                }
                Err(error) => {
                    crate::log_msg!("OTA manifest read error: {:?}", error);
                    OTA_RESULT_SIGNAL.signal(Err(()));
                }
            }
        }
        if !completed {
            if let Err(error) =
                crate::flash::invalidate_staging_manifest(&mut *flash, target_start).await
            {
                crate::log_msg!("OTA Consumer could not invalidate manifest: {:?}", error);
            }
        }

        crate::flash::stop_flashing();
        OTA_PIPE.clear();
    }
}

/// Entire A/B slot package, including its signed 4 KiB manifest page.
pub use crate::protocol::MAX_PACKAGE_SIZE as MAX_BIN_SIZE;
pub const SLOT_A_MANIFEST_ADDR: u32 = 0x0000_8000;
pub const SLOT_B_MANIFEST_ADDR: u32 = 0x0008_2000;

pub fn running_slot() -> u32 {
    if option_env!("PAGER_SLOT") == Some("B") {
        1
    } else {
        0
    }
}

pub fn inactive_slot() -> u32 {
    1 - running_slot()
}

pub fn inactive_slot_manifest_addr() -> u32 {
    if inactive_slot() == 0 {
        SLOT_A_MANIFEST_ADDR
    } else {
        SLOT_B_MANIFEST_ADDR
    }
}

/// NRF52840 USBD peripheral base address and control registers.
const USBD_BASE: usize = 0x4002_7000;
const USBD_ENABLE_REG: *mut u32 = (USBD_BASE + 0x500) as *mut u32;
const USBD_USBPULLUP_REG: *mut u32 = (USBD_BASE + 0x504) as *mut u32;

/// Detach USB long enough for host OS to discard USB state before reset.
pub async fn reset_after_usb_detach() -> ! {
    unsafe {
        core::ptr::write_volatile(USBD_USBPULLUP_REG, 0);
        core::ptr::write_volatile(USBD_ENABLE_REG, 0);
    }
    Timer::after(Duration::from_millis(3000)).await;
    crate::flash::reset_to_bootloader()
}
