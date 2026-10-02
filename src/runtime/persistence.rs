//! Durable snapshot task and rollback recovery.
use crate::{
    ble, diagnostics, faults, flash, idle_progress, log_msg, HEARTBEAT_STORAGE, STORAGE_ERRORS,
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
#[embassy_executor::task]
pub async fn persist_keyboard_state_task(
    flash_mutex: &'static Mutex<ThreadModeRawMutex, nrf_mpsl::Flash<'static>>,
    mut cursor: flash::StorageCursor,
) -> ! {
    let mut outcomes = ble::PersistOutcomes::default();
    loop {
        let signaled = idle_progress(HEARTBEAT_STORAGE, ble::PERSIST_STATE.wait()).await;
        if outcomes.result(signaled).is_some() {
            continue;
        }
        // No await between capturing the latest request and its snapshot.
        // Coalesced signals refer to changes covered by this same snapshot.
        let persist_sequence = ble::current_persist_sequence();
        ble::PERSIST_STATE.try_take();
        let (active_profile, bluetooth_enabled, bonds, cccd_flags, slot_names, device_name) =
            ble::KEYBOARD_STATE.lock(|state| {
                let state = state.borrow();
                (
                    state.active_profile,
                    state.bluetooth_enabled,
                    state.bonds.clone(),
                    state.cccd_flags,
                    state.slot_names.clone(),
                    state.device_name.clone(),
                )
            });
        let mut flash = flash_mutex.lock().await;
        if faults::take(3) {
            log_msg!("FAULT:STORAGE_STALL");
            core::future::pending::<()>().await;
        }
        match flash::save_persistent_state_cached(
            &mut *flash,
            &mut cursor,
            active_profile,
            bluetooth_enabled,
            &bonds,
            &cccd_flags,
            &slot_names,
            &device_name,
        )
        .await
        {
            Ok(()) => {
                ble::publish_state_changed();
                outcomes.complete(persist_sequence, true);
                ble::PERSIST_DONE.sender().send(outcomes);
            }
            Err(error) => {
                diagnostics::increment_saturated(&STORAGE_ERRORS);
                log_msg!("PERSIST_STATE:ERROR:{:?}", error);
                let persisted = match flash::scan_storage(&mut *flash).await {
                    Ok((recovered_cursor, state)) => {
                        cursor = recovered_cursor;
                        state
                    }
                    Err(error) => {
                        log_msg!("STORAGE:RECOVERY_READ_ERROR:{:?}", error);
                        cursor.persistent_state()
                    }
                };
                ble::KEYBOARD_STATE.lock(|state| {
                    let mut state = state.borrow_mut();
                    if let Some(persisted) = persisted {
                        state.active_profile = persisted.active_profile;
                        state.bluetooth_enabled = persisted.bluetooth_enabled;
                        state.bonds = persisted.bonds;
                        state.cccd_flags = persisted.cccd_flags;
                        state.slot_names = persisted.slot_names;
                        state.device_name = persisted.device_name;
                    } else {
                        state.active_profile = None;
                        state.bluetooth_enabled = false;
                        state.bonds = [None, None, None];
                        state.cccd_flags = [0; 3];
                        state.slot_names = Default::default();
                        state.device_name.clear();
                        let _ = state.device_name.push_str("Pager");
                    }
                    state.connected_profile = None;
                    state.pairing_mode = false;
                    state.fast_advertising = false;
                    state.hid_ready = false;
                    state.link_state = if state.bluetooth_enabled {
                        ble::BleLinkState::Idle
                    } else {
                        ble::BleLinkState::BluetoothOff
                    };
                });
                ble::publish_state_changed();
                // Rollback also discarded changes requested while flash/recovery
                // was awaiting. Mark that entire range terminally failed.
                let discarded_through = ble::current_persist_sequence();
                ble::PERSIST_STATE.try_take();
                outcomes.complete(discarded_through, false);
                ble::PERSIST_DONE.sender().send(outcomes);
                ble::STORAGE_RECOVERY.signal(());
            }
        }
    }
}
