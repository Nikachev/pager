//! Replaceable controller/session supervisor; GATT resources live for uptime.
use crate::{
    ble, faults, idle_progress, log_msg, signal_heartbeat, Irqs, HEARTBEAT_BLE, REQUIRED_HEARTBEATS,
};
use ble::Server;
use core::sync::atomic::Ordering;
use defmt::*;
use embassy_futures::select::{Either, Either3, Either4};
use embassy_nrf::{mode::Async, pac, peripherals::RNG, rng, Peri};
use embassy_time::{Duration, Timer};
use nrf_sdc::{self as sdc, mpsl::MultiprotocolServiceLayer};
use trouble_host::prelude::*;
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;
fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut rng::Rng<Async>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_ext_adv()
        .support_peripheral()
        .adv_count(3)?
        .peripheral_count(1)?
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            L2CAP_TXQ,
            L2CAP_RXQ,
        )?
        .build(p, rng, mpsl, mem)
}

fn ble_static_random_address(active_profile: Option<usize>) -> [u8; 6] {
    let low = pac::FICR.deviceaddr(0).read();
    let high = pac::FICR.deviceaddr(1).read();
    let mut addr = [
        (low & 0xFF) as u8,
        ((low >> 8) & 0xFF) as u8,
        ((low >> 16) & 0xFF) as u8,
        ((low >> 24) & 0xFF) as u8,
        (high & 0xFF) as u8,
        ((high >> 8) & 0xFF) as u8,
    ];
    addr[5] |= 0xC0;
    addr[0] ^= active_profile.map(|slot| slot as u8 + 1).unwrap_or(0);
    addr
}

fn advertised_device_name(base: &str, active_profile: Option<usize>) -> heapless::String<32> {
    let mut name = heapless::String::new();
    let _ = name.push_str(if base.is_empty() { "Pager" } else { base });
    if let Some(slot) = active_profile {
        let _ = core::fmt::write(&mut name, format_args!(" {}", slot + 1));
    }
    name
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
        state
            .active_profile
            .and_then(|slot| state.bonds[slot].clone())
    });
    if let Some(bond) = active_bond {
        if stack.add_bond_information(bond).is_err() {
            crate::log_msg!("BLE:BOND_SYNC_ERROR");
        }
    }
}

fn apply_ble_control_command(command: &ble::BleCommand) -> ble::Effects {
    let effects = ble::KEYBOARD_STATE.lock(|state| {
        state
            .borrow_mut()
            .session_event(ble::SessionEvent::Control(command))
    });
    if effects.changed {
        ble::publish_state_changed();
    }
    effects
}

fn is_ble_control_noop(command: &ble::BleCommand) -> bool {
    ble::KEYBOARD_STATE.lock(|state| state.borrow().is_noop(command))
}

async fn prepare_control(request: ble::BleRequest) -> Option<(u32, u64, ble::BleCommand)> {
    if faults::take(4) {
        crate::log_msg!("FAULT:BLE_STALL");
        core::future::pending::<()>().await;
    }
    if faults::take(6) {
        crate::log_msg!("FAULT:QUEUED_CONTROL_EXPIRY");
        Timer::after(Duration::from_secs(11)).await;
    }
    ble::start_control(request)
}

async fn finish_ble_control_command(id: u32, sequence: u32) {
    let persisted = ble::wait_persist(sequence).await;
    if faults::take(7) {
        log_msg!("FAULT:LATE_CONTROL_RESULT");
        Timer::after(Duration::from_secs(11)).await;
    }
    ble::complete_control(id, if persisted { 0 } else { 1 });
}

fn apply_session_event(event: ble::SessionEvent<'_>) -> ble::Effects {
    let effects = ble::KEYBOARD_STATE.lock(|state| state.borrow_mut().session_event(event));
    if effects.changed {
        ble::publish_state_changed();
    }
    if effects.persist {
        ble::request_persist();
    }
    effects
}

fn client_cccd_flags(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> Option<u8> {
    let table = server.get_client_att_table(conn.raw())?;
    Some(ble::cccd_flags([
        server
            .hid_service
            .input_keyboard
            .cccd_handle
            .and_then(|h| table.get(h)),
        server
            .hid_service
            .boot_input_keyboard
            .cccd_handle
            .and_then(|h| table.get(h)),
        server
            .battery_service
            .level
            .cccd_handle
            .and_then(|h| table.get(h)),
    ]))
}

async fn send_hid_report(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    report: &[u8; 8],
) -> bool {
    let mode = server.hid_service.protocol_mode.get(server).unwrap_or(1);
    // Trouble's send future owns its PDU until queue submission; cancellation
    // while waiting for flow control drops it without losing an enqueued report.
    embassy_time::with_timeout(Duration::from_millis(500), async {
        if mode == 0 {
            server
                .hid_service
                .boot_input_keyboard
                .notify(conn, report, true)
                .await
        } else {
            server
                .hid_service
                .input_keyboard
                .notify(conn, report, true)
                .await
        }
    })
    .await
    .is_ok_and(|result| result.is_ok())
}

pub async fn run(mpsl: &MultiprotocolServiceLayer<'_>, rng_peripheral: Peri<'_, RNG>) -> ! {
    let mut rng = rng::Rng::new(rng_peripheral, Irqs);
    let mut sdc_mem = sdc::Mem::<8192>::new();
    // Characteristic values generated by Trouble use one-shot StaticCell
    // storage. The GATT database therefore belongs to the firmware uptime, not
    // to a replaceable radio/controller session.
    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Pager",
        appearance: &appearance::human_interface_device::KEYBOARD,
    })) {
        Ok(server) => server,
        Err(error) => {
            crate::log_msg!("BLE:GATT_CONFIG_ERROR:{}", error);
            loop {
                crate::signal_heartbeat(crate::HEARTBEAT_BLE);
                Timer::after(Duration::from_secs(1)).await;
            }
        }
    };

    REQUIRED_HEARTBEATS.store(15, Ordering::Relaxed);
    info!("Pager HID & Web Server: supervisor started");
    loop {
        let bluetooth_enabled = ble::KEYBOARD_STATE.lock(|state| state.borrow().bluetooth_enabled);
        if !bluetooth_enabled {
            crate::signal_heartbeat(crate::HEARTBEAT_BLE);
            let request = idle_progress(HEARTBEAT_BLE, ble::BLE_COMMANDS.receive()).await;
            let Some((id, _deadline_ticks, command)) = prepare_control(request).await else {
                continue;
            };
            if is_ble_control_noop(&command) {
                ble::complete_control(id, 0);
                continue;
            }
            if apply_ble_control_command(&command).persist {
                let sequence = ble::request_persist();
                finish_ble_control_command(id, sequence).await;
            } else {
                ble::complete_control(id, 3);
            }
            continue;
        }

        // The SDC peripheral tokens are logically returned when
        // SoftdeviceController::drop() calls sdc_disable(). Embassy's singleton
        // token API cannot express that dynamic lifecycle, so the supervisor
        // reacquires these disjoint PPI tokens only after the preceding
        // controller and stack have been dropped.
        let sdc_p = unsafe {
            sdc::Peripherals::new(
                embassy_nrf::peripherals::PPI_CH17::steal(),
                embassy_nrf::peripherals::PPI_CH18::steal(),
                embassy_nrf::peripherals::PPI_CH20::steal(),
                embassy_nrf::peripherals::PPI_CH21::steal(),
                embassy_nrf::peripherals::PPI_CH22::steal(),
                embassy_nrf::peripherals::PPI_CH23::steal(),
                embassy_nrf::peripherals::PPI_CH24::steal(),
                embassy_nrf::peripherals::PPI_CH25::steal(),
                embassy_nrf::peripherals::PPI_CH26::steal(),
                embassy_nrf::peripherals::PPI_CH27::steal(),
                embassy_nrf::peripherals::PPI_CH28::steal(),
                embassy_nrf::peripherals::PPI_CH29::steal(),
            )
        };
        ble::STORAGE_RECOVERY.try_take();
        let sdc = match build_sdc(sdc_p, &mut rng, mpsl, &mut sdc_mem) {
            Ok(controller) => controller,
            Err(error) => {
                crate::log_msg!("BLE:CONTROLLER_START_ERROR:{:?}", error);
                Timer::after(Duration::from_millis(250)).await;
                continue;
            }
        };
        let mut radio_profile =
            crate::ble::KEYBOARD_STATE.lock(|state| state.borrow().active_profile);
        let address = Address::random(ble_static_random_address(radio_profile));
        crate::log_msg!("BLE:CONTROLLER_STARTED:{:?}", address);

        let mut resources: HostResources<DefaultPacketPool, 1, 2, 3> = HostResources::new();
        let stack = trouble_host::new(sdc, &mut resources)
            .set_random_address(address)
            .build();
        sync_active_bond(&stack);
        let mut runner = stack.runner();
        let mut peripheral = stack.peripheral();
        let mut prepared_profiles = [false; 3];
        let mut active_hid_id: Option<u32> = None;
        let session = async {
            loop {
                crate::signal_heartbeat(crate::HEARTBEAT_BLE);

                // The outer loop only runs without an active connection. Updating
                // the Security Manager here prevents a slot switch from removing
                // the old bond while its link is still being torn down.
                sync_active_bond(&stack);

                let (bluetooth_enabled, active_profile, pairing_mode, fast_advertising, peer) =
                    ble::KEYBOARD_STATE.lock(|state| {
                        let state = state.borrow();
                        let peer = state
                            .active_profile
                            .and_then(|slot| state.bonds[slot].as_ref())
                            .map(|bond| bond.identity.addr);
                        (
                            state.bluetooth_enabled,
                            state.active_profile,
                            state.pairing_mode,
                            state.fast_advertising,
                            peer,
                        )
                    });

                if active_profile != radio_profile {
                    radio_profile = active_profile;
                    crate::log_msg!("BLE:ACTIVE_ADVERTISING_SET_CHANGED");
                    Timer::after(Duration::from_secs(1)).await;
                    continue;
                }

                if !bluetooth_enabled
                    || active_profile.is_none()
                    || (peer.is_none() && !pairing_mode)
                {
                    ble::KEYBOARD_STATE.lock(|state| {
                        let mut state = state.borrow_mut();
                        state.link_state = if state.bluetooth_enabled {
                            ble::BleLinkState::Idle
                        } else {
                            ble::BleLinkState::BluetoothOff
                        };
                    });
                    let request = idle_progress(HEARTBEAT_BLE, ble::BLE_COMMANDS.receive()).await;
                    let Some((id, _deadline_ticks, command)) = prepare_control(request).await
                    else {
                        continue;
                    };
                    if is_ble_control_noop(&command) {
                        ble::complete_control(id, 0);
                        continue;
                    }
                    if apply_ble_control_command(&command).persist {
                        let sequence = ble::request_persist();
                        finish_ble_control_command(id, sequence).await;
                    } else {
                        ble::complete_control(id, 3);
                    }
                    continue;
                }

                crate::log_msg!("BLE:ADVERTISING");
                ble::KEYBOARD_STATE.lock(|state| {
                    let mut state = state.borrow_mut();
                    state.link_state = if state.pairing_mode {
                        ble::BleLinkState::Pairing
                    } else {
                        ble::BleLinkState::Advertising
                    };
                });
                // Hosts using BLE privacy reconnect from rotating private addresses.
                // Directed advertising to the stored identity address prevents those
                // reconnects on macOS and Android. Keep only the active slot's bond in
                // the resolving list and advertise undirected so that host-side HID
                // reconnection can discover Pager normally.
                let runtime_name = ble::KEYBOARD_STATE.lock(|state| {
                    advertised_device_name(&state.borrow().device_name, active_profile)
                });
                stack.set_runtime_local_address(Address::random(ble_static_random_address(
                    active_profile,
                )));
                let Some(payload) = pager::advertising::Payload::new(pairing_mode, &runtime_name)
                else {
                    crate::log_msg!("BLE:SCAN_DATA_ERROR");
                    Timer::after(Duration::from_secs(1)).await;
                    continue;
                };
                let advertisement = Advertisement::ConnectableScannableUndirected {
                    adv_data: &payload.advertising[..payload.advertising_len],
                    scan_data: &payload.scan_response[..payload.scan_response_len],
                };
                let mut advertisement_params = AdvertisementParameters::default();
                let phase_timeout = if pairing_mode {
                    advertisement_params.interval_min = Duration::from_millis(50);
                    advertisement_params.interval_max = Duration::from_millis(80);
                    Duration::from_secs(120)
                } else if fast_advertising {
                    advertisement_params.interval_min = Duration::from_millis(50);
                    advertisement_params.interval_max = Duration::from_millis(80);
                    Duration::from_secs(10)
                } else if !pairing_mode && peer.is_some() {
                    advertisement_params.interval_min = Duration::from_secs(1);
                    advertisement_params.interval_max = Duration::from_millis(1200);
                    Duration::from_secs(3600)
                } else {
                    Duration::from_secs(3600)
                };
                let Some(profile) = active_profile else {
                    crate::log_msg!("BLE:ADVERTISE_WITHOUT_ACTIVE_SLOT");
                    continue;
                };
                let advertisement_started = embassy_time::Instant::now();
                let advertiser_result = if prepared_profiles[profile] {
                    peripheral
                        .advertise_ext_prepared(profile as u8, &advertisement_params, advertisement)
                        .await
                } else {
                    let sets = [AdvertisementSet {
                        params: advertisement_params,
                        data: advertisement,
                        address: Some(BdAddr::new(ble_static_random_address(active_profile))),
                    }];
                    let mut handles = AdvertisementSet::handles(&sets);
                    peripheral
                        .advertise_ext_from_handle(profile as u8, &sets, &mut handles)
                        .await
                };
                let advertiser = match advertiser_result {
                    Ok(advertiser) => advertiser,
                    Err(error) => {
                        crate::log_msg!("BLE:ADVERTISE_ERROR:{:?}", error);
                        Timer::after(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                let advertisement_enabled = embassy_time::Instant::now();
                crate::log_msg!(
                    "BLE:ADV_ENABLED:slot={}:uptime_ms={}:setup_ms={}",
                    profile + 1,
                    advertisement_enabled.as_millis(),
                    (advertisement_enabled - advertisement_started).as_millis()
                );
                prepared_profiles[profile] = true;
                info!("Pager: waiting for connection...");
                let conn = match embassy_futures::select::select3(
                    idle_progress(HEARTBEAT_BLE, advertiser.accept()),
                    ble::BLE_COMMANDS.receive(),
                    Timer::after(phase_timeout),
                )
                .await
                {
                    Either3::First(Ok(conn)) => conn,
                    Either3::First(Err(error)) => {
                        crate::log_msg!("BLE:ACCEPT_ERROR:{:?}", error);
                        continue;
                    }
                    Either3::Second(request) => {
                        let Some((id, _deadline_ticks, command)) = prepare_control(request).await
                        else {
                            continue;
                        };
                        if is_ble_control_noop(&command) {
                            ble::complete_control(id, 0);
                            continue;
                        }
                        if apply_ble_control_command(&command).persist {
                            let sequence = ble::request_persist();
                            finish_ble_control_command(id, sequence).await;
                        } else {
                            ble::complete_control(id, 3);
                        }
                        continue;
                    }
                    Either3::Third(()) => {
                        if pairing_mode {
                            ble::KEYBOARD_STATE.lock(|state| {
                                let mut state = state.borrow_mut();
                                state.bluetooth_enabled = false;
                                state.active_profile = None;
                                state.connected_profile = None;
                                state.hid_ready = false;
                                state.pairing_mode = false;
                                state.fast_advertising = false;
                                state.link_state = ble::BleLinkState::BluetoothOff;
                            });
                            ble::publish_state_changed();
                            ble::request_persist();
                        } else if fast_advertising {
                            ble::KEYBOARD_STATE.lock(|state| {
                                state.borrow_mut().fast_advertising = false;
                            });
                        }
                        continue;
                    }
                };
                ble::KEYBOARD_STATE.lock(|state| {
                    state.borrow_mut().link_state = ble::BleLinkState::Connecting;
                });
                ble::publish_state_changed();
                let Some(connection_profile) = active_profile else {
                    crate::log_msg!("BLE:CONNECTION_WITHOUT_ACTIVE_SLOT");
                    conn.disconnect();
                    continue;
                };
                let pairing_mode = ble::KEYBOARD_STATE.lock(|state| state.borrow().pairing_mode);
                let _ = conn.set_bondable(pairing_mode);
                let active_bond = ble::KEYBOARD_STATE
                    .lock(|state| state.borrow().bonds[connection_profile].clone());
                if let Some(ref bond) = active_bond {
                    if conn.peer_identity() != bond.identity {
                        crate::log_msg!("BLE:REJECT_NON_ACTIVE_PEER");
                        conn.disconnect();
                        continue;
                    }
                    if let Err(error) = conn.request_security() {
                        crate::log_msg!("BLE:SECURITY_REQUEST_ERROR:{:?}", error);
                    } else {
                        crate::log_msg!("BLE:SECURITY_REQUESTED");
                    }
                }
                let conn = match conn.with_attribute_server(&server) {
                    Ok(conn) => conn,
                    Err(error) => {
                        crate::log_msg!("BLE:GATT_SERVER_ERROR:{:?}", error);
                        continue;
                    }
                };
                let (cccd_flags, healed_cccd) = ble::KEYBOARD_STATE.lock(|state| {
                    let mut state = state.borrow_mut();
                    let previous = state.cccd_flags[connection_profile];
                    let healed = ble::bonded_hid_cccd_flags(
                        state.bonds[connection_profile].is_some(),
                        previous,
                    );
                    state.cccd_flags[connection_profile] = healed;
                    (healed, healed != previous)
                });
                if healed_cccd {
                    crate::log_msg!("BLE:HEALED_BONDED_HID_CCCD");
                    ble::request_persist();
                }
                if cccd_flags != 0 {
                    if let Some(mut table) = server.get_client_att_table(conn.raw()) {
                        for (flag, handle) in [
                            (1, server.hid_service.input_keyboard.cccd_handle),
                            (2, server.hid_service.boot_input_keyboard.cccd_handle),
                            (4, server.battery_service.level.cccd_handle),
                        ] {
                            if cccd_flags & flag != 0 {
                                if let Some(handle) = handle {
                                    let _ = table.write(handle, 0, &[1, 0]);
                                }
                            }
                        }
                        server.set_client_att_table(conn.raw(), &table.view());
                    }
                }
                info!("Pager: connection established!");
                crate::log_msg!("BLE:CONNECTED");
                apply_session_event(ble::SessionEvent::Connecting(connection_profile));

                let mut persist_after_disconnect = false;
                let mut hid_job: Option<crate::hid::Job> = None;
                loop {
                    signal_heartbeat(HEARTBEAT_BLE);
                    let hid_due = hid_job
                        .as_ref()
                        .map(|job| embassy_time::Instant::from_ticks(job.due()))
                        .unwrap_or_else(|| {
                            embassy_time::Instant::now() + Duration::from_secs(3600)
                        });
                    match embassy_futures::select::select4(
                        conn.next(),
                        ble::BLE_COMMANDS.receive(),
                        Timer::after(Duration::from_secs(1)),
                        Timer::at(hid_due),
                    )
                    .await
                    {
                        Either4::First(event) => match event {
                            GattConnectionEvent::Disconnected { reason } => {
                                if let Some(job) = hid_job.take() {
                                    active_hid_id = None;
                                    ble::complete_control(job.id, 3);
                                }
                                info!("Pager: disconnected {:?}", reason);
                                crate::log_msg!("BLE:DISCONNECTED:{:?}", reason);
                                if let Some(flags) = client_cccd_flags(&server, &conn) {
                                    ble::KEYBOARD_STATE.lock(|state| {
                                        state.borrow_mut().cccd_flags[connection_profile] = flags
                                    });
                                    ble::request_persist();
                                }
                                apply_session_event(ble::SessionEvent::Disconnected(
                                    connection_profile,
                                ));
                                break;
                            }
                            GattConnectionEvent::PairingComplete {
                                security_level,
                                bond,
                            } => {
                                crate::log_msg!(
                                    "BLE:PAIRING_COMPLETE:LEVEL:{:?}:BONDED:{}",
                                    security_level,
                                    bond.is_some()
                                );
                                info!(
                                    "Pager: pairing complete! Level: {:?}, bonded: {}",
                                    security_level,
                                    bond.is_some()
                                );
                                apply_session_event(ble::SessionEvent::Secured {
                                    slot: connection_profile,
                                    bond,
                                });
                                persist_after_disconnect = true;
                            }
                            GattConnectionEvent::PairingFailed(err) => {
                                match err {
                                    trouble_host::Error::Security(reason) => {
                                        crate::log_msg!("BLE:PAIRING_FAILED:SECURITY:{:?}", reason)
                                    }
                                    _ => crate::log_msg!("BLE:PAIRING_FAILED:OTHER"),
                                }
                                if apply_session_event(ble::SessionEvent::PairingFailed(
                                    connection_profile,
                                ))
                                .disconnect
                                {
                                    conn.raw().disconnect();
                                }
                            }
                            GattConnectionEvent::Encrypted {
                                security_level,
                                bond,
                            } => {
                                crate::log_msg!(
                                    "BLE:ENCRYPTED:LEVEL:{:?}:BONDED:{}",
                                    security_level,
                                    bond.is_some()
                                );
                                info!(
                                    "Pager: connection encrypted! Level: {:?}, bonded: {}",
                                    security_level,
                                    bond.is_some()
                                );
                                apply_session_event(ble::SessionEvent::Secured {
                                    slot: connection_profile,
                                    bond,
                                });
                                persist_after_disconnect = true;
                            }
                            GattConnectionEvent::Gatt { event } => {
                                if let GattEvent::NotAllowed(req) = &event {
                                    crate::log_msg!("BLE:GATT_NOT_ALLOWED:handle={}", req.handle());
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
                        Either4::Second(request) => {
                            let Some((id, deadline_ticks, command)) =
                                prepare_control(request).await
                            else {
                                continue;
                            };
                            match command {
                                ble::BleCommand::TypeString(text) => {
                                    if hid_job.is_some()
                                        || !ble::KEYBOARD_STATE.lock(|s| s.borrow().hid_ready)
                                    {
                                        ble::complete_control(id, 3);
                                    } else {
                                        active_hid_id = Some(id);
                                        hid_job = Some(crate::hid::Job::new(
                                            id,
                                            text,
                                            deadline_ticks,
                                            embassy_time::Instant::now().as_ticks(),
                                            Duration::from_millis(12).as_ticks(),
                                        ));
                                    }
                                }
                                command => {
                                    if let Some(job) = hid_job.take() {
                                        active_hid_id = None;
                                        if job.needs_release() {
                                            let _ = send_hid_report(&server, &conn, &[0; 8]).await;
                                        }
                                        ble::complete_control(job.id, 3);
                                    }
                                    if is_ble_control_noop(&command) {
                                        ble::complete_control(id, 0);
                                        continue;
                                    }
                                    if let Some(flags) = client_cccd_flags(&server, &conn) {
                                        ble::KEYBOARD_STATE.lock(|state| {
                                            state.borrow_mut().cccd_flags[connection_profile] =
                                                flags
                                        });
                                    }
                                    let effects = apply_ble_control_command(&command);
                                    let disconnect_required = effects.disconnect;
                                    if effects.persist {
                                        let sequence = ble::request_persist();
                                        finish_ble_control_command(id, sequence).await;
                                    } else {
                                        ble::complete_control(id, 3);
                                    }
                                    if disconnect_required {
                                        conn.raw().disconnect();
                                        // `disconnect()` only queues the HCI command. Keep polling the
                                        // connection until Disconnection Complete is consumed; merely
                                        // polling `is_connected()` does not drive the per-connection
                                        // event queue in trouble-host.
                                        loop {
                                            crate::signal_heartbeat(crate::HEARTBEAT_BLE);
                                            match embassy_futures::select::select(
                                                conn.next(),
                                                Timer::after(Duration::from_secs(3)),
                                            )
                                            .await
                                            {
                                                Either::First(
                                                    GattConnectionEvent::Disconnected { reason },
                                                ) => {
                                                    crate::log_msg!(
                                                        "BLE:DISCONNECTED_FOR_SLOT_SWITCH:{:?}",
                                                        reason
                                                    );
                                                    // Nordic SDC can assert if a new connectable
                                                    // advertiser/controller session starts in the
                                                    // immediate tail of Disconnection Complete.
                                                    // Keep USB responsive while the radio settles.
                                                    Timer::after(Duration::from_secs(2)).await;
                                                    break;
                                                }
                                                Either::First(GattConnectionEvent::Gatt {
                                                    event,
                                                }) => {
                                                    if let Ok(reply) = event.accept() {
                                                        reply.send().await;
                                                    }
                                                }
                                                Either::First(_) => {}
                                                Either::Second(()) => {
                                                    crate::log_msg!("BLE:DISCONNECT_TIMEOUT");
                                                    break;
                                                }
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        Either4::Fourth(()) => {
                            let Some(job) = hid_job.as_mut() else {
                                continue;
                            };
                            match job.advance(embassy_time::Instant::now().as_ticks()) {
                                crate::hid::Step::Complete(result) => {
                                    let id = job.id;
                                    hid_job = None;
                                    active_hid_id = None;
                                    ble::complete_control(id, result);
                                }
                                crate::hid::Step::Report(report) => {
                                    if send_hid_report(&server, &conn, &report).await {
                                        job.report_sent(embassy_time::Instant::now().as_ticks());
                                    } else {
                                        let id = job.id;
                                        // Best effort release is bounded even if notification flow control failed.
                                        let _ = send_hid_report(&server, &conn, &[0; 8]).await;
                                        hid_job = None;
                                        active_hid_id = None;
                                        ble::complete_control(id, 3);
                                    }
                                }
                            }
                        }
                        Either4::Third(()) => {
                            let ready = client_cccd_flags(&server, &conn)
                                .is_some_and(|flags| flags & 3 != 0);
                            apply_session_event(ble::SessionEvent::HidReady {
                                slot: connection_profile,
                                ready,
                            });
                        }
                    }
                }
                if persist_after_disconnect {
                    ble::request_persist();
                }
            }
        };

        match embassy_futures::select::select3(runner.run(), session, ble::STORAGE_RECOVERY.wait())
            .await
        {
            Either3::First(result) => match result {
                Ok(()) => crate::log_msg!("BLE:RUNNER_STOPPED:OK"),
                Err(trouble_host::BleHostError::Controller(error)) => {
                    crate::log_msg!("BLE:RUNNER_STOPPED:CONTROLLER:{}", i32::from(error))
                }
                Err(trouble_host::BleHostError::BleHost(trouble_host::Error::Hci(error))) => {
                    crate::log_msg!("BLE:RUNNER_STOPPED:HCI:{}", u8::from(error))
                }
                Err(trouble_host::BleHostError::BleHost(error)) => crate::log_msg!(
                    "BLE:RUNNER_STOPPED:HOST:{}",
                    match error {
                        trouble_host::Error::Disconnected => "DISCONNECTED",
                        trouble_host::Error::NoPermits => "NO_PERMITS",
                        trouble_host::Error::Busy => "BUSY",
                        trouble_host::Error::InvalidState => "INVALID_STATE",
                        trouble_host::Error::OutOfMemory => "OUT_OF_MEMORY",
                        trouble_host::Error::Timeout => "TIMEOUT",
                        trouble_host::Error::NotFound => "NOT_FOUND",
                        trouble_host::Error::Security(_) => "SECURITY",
                        _ => "OTHER",
                    }
                ),
            },
            Either3::Second(()) => crate::log_msg!("BLE:SESSION_STOPPED"),
            Either3::Third(()) => crate::log_msg!("BLE:STORAGE_RECOVERY_RESTART"),
        }
        if let Some(id) = active_hid_id.take() {
            ble::complete_control(id, 3);
        }
        apply_session_event(ble::SessionEvent::ControllerStopped);
        // Drop order matters: handles/futures, Stack/HostState, then the SDC
        // controller owned by Stack. USB is driven by separate spawned tasks.
        drop(stack);
        crate::log_msg!("BLE:CONTROLLER_STOPPED");
        // SDC may assert when a new connectable advertiser is started in the
        // immediate tail of controller shutdown. USB continues running while
        // the radio/controller settle.
        Timer::after(Duration::from_secs(2)).await;
    }
}
