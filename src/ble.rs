use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(not(target_arch = "arm"))]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as ThreadModeRawMutex;
#[cfg(target_arch = "arm")]
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use trouble_host::prelude::*;

// ---------------------------------------------------------------------------
// GATT Server and Services with Security & Encryption (trouble-host)
// ---------------------------------------------------------------------------

#[gatt_server(attribute_table_size = 128)]
pub struct Server {
    pub hid_service: HidService,
    pub battery_service: BatteryService,
}

static HID_REPORT_DESCRIPTOR: [u8; 67] = [
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x01, 0x81,
    0x03, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x25, 0x01, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, 0x95,
    0x03, 0x91, 0x03, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x06,
    0x81, 0x00, 0xc0,
];

#[gatt_service(uuid = "1812")]
pub struct HidService {
    // HID discovery metadata must be readable before pairing. macOS uses the
    // report map to decide that this is a keyboard and initiate its security
    // flow; protecting it makes CoreBluetooth hide the characteristics.
    #[characteristic(uuid = "2a4a", read, value = [0x11, 0x01, 0x00, 0x03])]
    pub hid_info: [u8; 4],

    #[characteristic(uuid = "2a4b", read, value = HID_REPORT_DESCRIPTOR)]
    pub report_map: [u8; 67],

    #[characteristic(uuid = "2a4c", write_without_response, permissions(encrypted))]
    pub hid_control_point: u8,

    #[characteristic(uuid = "2a4e", read, write_without_response, value = 1)]
    pub protocol_mode: u8,

    #[descriptor(uuid = "2908", read = encrypted, value = [0u8, 1u8])]
    #[characteristic(uuid = "2a4d", read, notify, permissions(encrypted))]
    pub input_keyboard: [u8; 8],

    #[characteristic(uuid = "2a22", read, notify, permissions(encrypted))]
    pub boot_input_keyboard: [u8; 8],

    #[descriptor(uuid = "2908", read = encrypted, value = [0u8, 2u8])]
    #[characteristic(
        uuid = "2a4d",
        read,
        write,
        write_without_response,
        permissions(encrypted)
    )]
    pub output_keyboard: [u8; 1],
}

#[gatt_service(uuid = "180f")]
pub struct BatteryService {
    // Development placeholder until a battery and ADC divider are connected.
    #[characteristic(uuid = "2a19", read, notify, value = 13, permissions(encrypted))]
    pub level: u8,
}

pub struct KeyboardState {
    pub bonds: [Option<BondInformation>; 3],
    pub cccd_flags: [u8; 3],
    pub slot_names: [heapless::String<32>; 3],
    pub device_name: heapless::String<24>,
    pub active_profile: Option<usize>,
    pub connected_profile: Option<usize>,
    pub bluetooth_enabled: bool,
    pub link_state: BleLinkState,
    pub pairing_mode: bool,
    pub fast_advertising: bool,
    pub hid_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BleLinkState {
    BluetoothOff = 0,
    Idle = 1,
    Advertising = 2,
    Pairing = 3,
    Connecting = 4,
    Connected = 5,
    Disconnecting = 6,
}

pub fn format_default_peer_name(raw_addr: &[u8]) -> heapless::String<32> {
    let mut s = heapless::String::new();
    if raw_addr.len() == 6 {
        let _ = core::fmt::write(
            &mut s,
            format_args!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                raw_addr[5], raw_addr[4], raw_addr[3], raw_addr[2], raw_addr[1], raw_addr[0]
            ),
        );
    } else {
        let _ = core::fmt::write(&mut s, format_args!("Paired Device"));
    }
    s
}

/// Recover semantic HID subscriptions for a trusted peer when an older
/// development record contains a bond but no CCCD state. Hosts such as macOS
/// may assume this bonded peripheral state survived reboot and not write it
/// again themselves.
pub fn bonded_hid_cccd_flags(bonded: bool, flags: u8) -> u8 {
    if bonded && flags & 0b11 == 0 {
        flags | 0b11
    } else {
        flags
    }
}

pub static KEYBOARD_STATE: SyncMutex<ThreadModeRawMutex, RefCell<KeyboardState>> =
    SyncMutex::new(RefCell::new(KeyboardState {
        bonds: [None, None, None],
        cccd_flags: [0; 3],
        slot_names: [
            heapless::String::new(),
            heapless::String::new(),
            heapless::String::new(),
        ],
        device_name: heapless::String::new(),
        active_profile: None,
        connected_profile: None,
        bluetooth_enabled: false,
        link_state: BleLinkState::BluetoothOff,
        pairing_mode: false,
        fast_advertising: false,
        hid_ready: false,
    }));

// TYPE_TEXT deliberately owns its fixed-capacity buffer: the command queue is
// the single job owner and no allocator/second payload queue can get out of sync.
#[allow(clippy::large_enum_variant)]
pub enum BleCommand {
    ActivateSlot(usize),
    CancelPairing,
    ClearSlot(usize),
    SetBluetoothEnabled(bool),
    SetDeviceName(heapless::String<24>),
    SetSlotName(usize, heapless::String<32>),
    FactoryReset,
    TypeString(heapless::String<256>),
}

impl KeyboardState {
    /// Apply the in-memory part of a control command. Persistence, radio and
    /// USB effects are deliberately owned by the caller.
    pub fn reduce(&mut self, command: &BleCommand) -> bool {
        match command {
            BleCommand::ActivateSlot(slot) if *slot < self.bonds.len() => {
                if !self.bluetooth_enabled {
                    return true;
                }
                let occupied = self.bonds[*slot].is_some();
                if self.active_profile == Some(*slot) && occupied {
                    return true;
                }
                self.active_profile = Some(*slot);
                self.connected_profile = None;
                self.hid_ready = false;
                self.pairing_mode = !occupied;
                self.fast_advertising = occupied;
                self.link_state = if occupied {
                    BleLinkState::Advertising
                } else {
                    BleLinkState::Pairing
                };
                true
            }
            BleCommand::CancelPairing => {
                self.enter_bluetooth_off();
                true
            }
            BleCommand::ClearSlot(slot) if *slot < self.bonds.len() => {
                self.bonds[*slot] = None;
                self.cccd_flags[*slot] = 0;
                self.slot_names[*slot].clear();
                if self.active_profile == Some(*slot) {
                    self.connected_profile = None;
                    self.hid_ready = false;
                    self.pairing_mode = false;
                    self.fast_advertising = false;
                    self.link_state = BleLinkState::Idle;
                }
                true
            }
            BleCommand::SetBluetoothEnabled(enabled) => {
                self.bluetooth_enabled = *enabled;
                self.active_profile = None;
                self.connected_profile = None;
                self.hid_ready = false;
                self.pairing_mode = false;
                self.fast_advertising = false;
                self.link_state = if *enabled {
                    BleLinkState::Idle
                } else {
                    BleLinkState::BluetoothOff
                };
                true
            }
            BleCommand::SetDeviceName(name) => {
                self.device_name.clear();
                self.device_name.push_str(name).is_ok()
            }
            BleCommand::SetSlotName(slot, name)
                if *slot < self.bonds.len() && self.bonds[*slot].is_some() =>
            {
                self.slot_names[*slot].clear();
                self.slot_names[*slot].push_str(name).is_ok()
            }
            BleCommand::FactoryReset => {
                self.bonds = [None, None, None];
                self.cccd_flags = [0; 3];
                self.slot_names = Default::default();
                self.device_name.clear();
                let _ = self.device_name.push_str("Pager");
                self.enter_bluetooth_off();
                true
            }
            BleCommand::TypeString(_) => false,
            _ => false,
        }
    }

    pub fn is_noop(&self, command: &BleCommand) -> bool {
        matches!(
            command,
            BleCommand::ActivateSlot(slot)
                if *slot < self.bonds.len()
                    && self.active_profile == Some(*slot)
                    && self.bonds[*slot].is_some()
        ) || matches!(
            command,
            BleCommand::SetBluetoothEnabled(enabled) if self.bluetooth_enabled == *enabled
        )
    }

    fn enter_bluetooth_off(&mut self) {
        self.bluetooth_enabled = false;
        self.active_profile = None;
        self.connected_profile = None;
        self.hid_ready = false;
        self.pairing_mode = false;
        self.fast_advertising = false;
        self.link_state = BleLinkState::BluetoothOff;
    }
}

pub fn default_device_name() -> heapless::String<32> {
    let mut name = heapless::String::new();
    let _ = name.push_str("Pager");
    name
}

pub static BLE_COMMANDS: Channel<ThreadModeRawMutex, BleRequest, 8> = Channel::new();
pub static STORAGE_RECOVERY: Signal<ThreadModeRawMutex, ()> = Signal::new();
pub static PERSIST_STATE: Signal<ThreadModeRawMutex, u32> = Signal::new();
pub static PERSIST_DONE: Watch<ThreadModeRawMutex, PersistOutcomes, 2> = Watch::new();

pub fn persist_covers(completed: u32, requested: u32) -> bool {
    completed.wrapping_sub(requested) < 0x8000_0000
}

/// Bounded terminal batch history. Evicted results fail closed rather than
/// being inferred from a later unrelated successful snapshot.
#[derive(Clone, Copy)]
struct PersistBatch {
    first: u32,
    last: u32,
    success: bool,
}

#[derive(Clone, Copy, Default)]
pub struct PersistOutcomes {
    batches: [Option<PersistBatch>; 8],
    completed: Option<u32>,
}

impl PersistOutcomes {
    pub fn complete(&mut self, through: u32, success: bool) {
        let first = self.completed.unwrap_or(0).wrapping_add(1);
        // A duplicate/stale signal cannot rewrite a terminal failure as success.
        if self
            .completed
            .is_some_and(|last| persist_covers(last, through))
        {
            return;
        }
        self.batches.rotate_right(1);
        self.batches[0] = Some(PersistBatch {
            first,
            last: through,
            success,
        });
        self.completed = Some(through);
    }

    pub fn result(&self, requested: u32) -> Option<bool> {
        for batch in self.batches.iter().flatten() {
            if requested.wrapping_sub(batch.first) <= batch.last.wrapping_sub(batch.first) {
                return Some(batch.success);
            }
        }
        self.completed
            .filter(|last| persist_covers(*last, requested))
            .map(|_| false)
    }
}

#[cfg(target_arch = "arm")]
pub async fn wait_persist(sequence: u32) -> bool {
    let Some(mut receiver) = PERSIST_DONE.receiver() else {
        return false;
    };
    embassy_time::with_timeout(
        embassy_time::Duration::from_secs(5),
        receiver.get_and(|outcomes| outcomes.result(sequence).is_some()),
    )
    .await
    .ok()
    .and_then(|outcomes| outcomes.result(sequence))
    .unwrap_or(false)
}

pub fn current_persist_sequence() -> u32 {
    PERSIST_SEQUENCE.load(Ordering::Relaxed)
}
static PERSIST_SEQUENCE: AtomicU32 = AtomicU32::new(0);
pub static BLE_CONTROL_RESULT: Watch<ThreadModeRawMutex, (u32, u8), 1> = Watch::new();
static CONTROL_SEQUENCE: AtomicU32 = AtomicU32::new(0);

pub struct BleRequest {
    pub id: u32,
    pub deadline_ticks: u64,
    pub command: BleCommand,
}

impl BleRequest {
    pub fn expired(&self, now_ticks: u64) -> bool {
        now_ticks >= self.deadline_ticks
    }
}

pub fn next_control_id() -> u32 {
    loop {
        let id = CONTROL_SEQUENCE
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if id != 0 {
            return id;
        }
    }
}

static TYPE_CONTROL_ID: AtomicU32 = AtomicU32::new(0);
pub static TYPE_CONTROL_RESULT: Signal<ThreadModeRawMutex, (u32, u8)> = Signal::new();
pub fn register_type_control(id: u32) -> bool {
    if TYPE_CONTROL_ID
        .compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    TYPE_CONTROL_RESULT.try_take();
    true
}
pub fn finish_type_control(id: u32) {
    let _ = TYPE_CONTROL_ID.compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed);
}
pub fn complete_control(id: u32, result: u8) {
    if TYPE_CONTROL_ID.load(Ordering::Relaxed) == id {
        TYPE_CONTROL_RESULT.signal((id, result));
    } else {
        BLE_CONTROL_RESULT.sender().send((id, result));
    }
}

#[cfg(target_arch = "arm")]
pub fn start_control(request: BleRequest) -> Option<(u32, u64, BleCommand)> {
    if request.expired(embassy_time::Instant::now().as_ticks()) {
        complete_control(request.id, 1);
        None
    } else {
        Some((request.id, request.deadline_ticks, request.command))
    }
}
pub static COMMAND_HIGH_WATER: AtomicU32 = AtomicU32::new(0);
pub static DROPPED_COMMANDS: AtomicU32 = AtomicU32::new(0);
pub static STATE_REVISION: AtomicU32 = AtomicU32::new(0);
pub static STATE_EVENTS: PubSubChannel<ThreadModeRawMutex, u32, 8, 2, 1> = PubSubChannel::new();

pub fn publish_state_changed() -> u32 {
    let revision = STATE_REVISION
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    STATE_EVENTS
        .immediate_publisher()
        .publish_immediate(revision);
    revision
}

pub fn request_persist() -> u32 {
    let sequence = PERSIST_SEQUENCE
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    PERSIST_STATE.signal(sequence);
    sequence
}

pub fn try_send_command(command: BleRequest) -> bool {
    if BLE_COMMANDS.try_send(command).is_ok() {
        COMMAND_HIGH_WATER.fetch_max(BLE_COMMANDS.len() as u32, Ordering::Relaxed);
        true
    } else {
        crate::diagnostics::increment_saturated(&DROPPED_COMMANDS);
        false
    }
}

pub fn ascii_to_hid(c: char) -> Option<(u8, u8)> {
    let b = c as u8;
    if b >= 128 {
        return None;
    }
    match c {
        'a'..='z' => Some((0x00, (b - b'a') + 0x04)),
        'A'..='Z' => Some((0x02, (b - b'A') + 0x04)),
        '1'..='9' => Some((0x00, (b - b'1') + 0x1E)),
        '0' => Some((0x00, 0x27)),
        '\n' | '\r' => Some((0x00, 0x28)),
        ' ' => Some((0x00, 0x2C)),
        '!' => Some((0x02, 0x1E)),
        '@' => Some((0x02, 0x1F)),
        '#' => Some((0x02, 0x20)),
        '$' => Some((0x02, 0x21)),
        '%' => Some((0x02, 0x22)),
        '^' => Some((0x02, 0x23)),
        '&' => Some((0x02, 0x24)),
        '*' => Some((0x02, 0x25)),
        '(' => Some((0x02, 0x26)),
        ')' => Some((0x02, 0x27)),
        '-' => Some((0x00, 0x2D)),
        '_' => Some((0x02, 0x2D)),
        '=' => Some((0x00, 0x2E)),
        '+' => Some((0x02, 0x2E)),
        '[' => Some((0x00, 0x2F)),
        '{' => Some((0x02, 0x2F)),
        ']' => Some((0x00, 0x30)),
        '}' => Some((0x02, 0x30)),
        '\\' => Some((0x00, 0x31)),
        '|' => Some((0x02, 0x31)),
        ';' => Some((0x00, 0x33)),
        ':' => Some((0x02, 0x33)),
        '\'' => Some((0x00, 0x34)),
        '"' => Some((0x02, 0x34)),
        '`' => Some((0x00, 0x35)),
        '~' => Some((0x02, 0x35)),
        ',' => Some((0x00, 0x36)),
        '<' => Some((0x02, 0x36)),
        '.' => Some((0x00, 0x37)),
        '>' => Some((0x02, 0x37)),
        '/' => Some((0x00, 0x38)),
        '?' => Some((0x02, 0x38)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturated_queue_preserves_order_and_rejects_only_the_new_request() {
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        let queue: Channel<NoopRawMutex, BleRequest, 8> = Channel::new();
        for id in 1..=8 {
            assert!(queue
                .try_send(BleRequest {
                    id,
                    deadline_ticks: 100,
                    command: BleCommand::CancelPairing
                })
                .is_ok());
        }
        let rejected = queue
            .try_send(BleRequest {
                id: 9,
                deadline_ticks: 100,
                command: BleCommand::FactoryReset,
            })
            .err()
            .unwrap();
        let embassy_sync::channel::TrySendError::Full(rejected) = rejected;
        assert_eq!(rejected.id, 9);
        assert_eq!(queue.len(), 8);
        for id in 1..=8 {
            assert_eq!(queue.try_receive().unwrap().id, id);
        }
        assert!(queue.try_receive().is_err());
    }

    #[test]
    fn pending_request_expires_at_deadline() {
        let request = BleRequest {
            id: 7,
            deadline_ticks: 100,
            command: BleCommand::FactoryReset,
        };
        assert!(!request.expired(99));
        assert!(request.expired(100));
        assert!(request.expired(101));
    }

    #[test]
    fn late_result_cannot_finish_next_request() {
        use core::future::Future;
        use core::task::{Context, Poll};
        let watch = Watch::<embassy_sync::blocking_mutex::raw::NoopRawMutex, (u32, u8), 1>::new();
        let mut receiver = watch.receiver().unwrap();
        // Request 7 timed out; request 8 is now waiting.
        watch.sender().send((7, 0));
        let mut waiting = core::pin::pin!(receiver.get_and(|(id, _)| *id == 8));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        watch.sender().send((7, 3));
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        watch.sender().send((8, 0));
        assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready((8, 0)));
    }

    fn state() -> KeyboardState {
        KeyboardState {
            bonds: [None, None, None],
            cccd_flags: [0; 3],
            slot_names: Default::default(),
            device_name: heapless::String::try_from("Pager").unwrap(),
            active_profile: None,
            connected_profile: None,
            bluetooth_enabled: false,
            link_state: BleLinkState::BluetoothOff,
            pairing_mode: false,
            fast_advertising: false,
            hid_ready: false,
        }
    }

    #[test]
    fn persistence_ack_is_broadcast_and_sequence_filtered() {
        use embassy_sync::blocking_mutex::raw::NoopRawMutex;
        let watch: Watch<NoopRawMutex, PersistOutcomes, 2> = Watch::new();
        let mut first = watch.receiver().unwrap();
        let mut second = watch.receiver().unwrap();
        let mut outcomes = PersistOutcomes::default();
        outcomes.complete(9, true);
        watch.sender().send(outcomes);
        let (a, b) = futures::executor::block_on(async {
            futures::join!(
                first.get_and(|o| o.result(8).is_some()),
                second.get_and(|o| o.result(9).is_some())
            )
        });
        assert_eq!(a.result(8), Some(true));
        assert_eq!(b.result(9), Some(true));
        assert_eq!(b.result(10), None);
        assert!(persist_covers(0, u32::MAX));
        assert!(!persist_covers(u32::MAX, 0));
    }

    #[test]
    fn rollback_failure_survives_later_success_and_stale_signal() {
        let mut outcomes = PersistOutcomes::default();
        outcomes.complete(2, true);
        // Commit 3 failed; requests 4 and 5 arrived during flash I/O and
        // their runtime changes were also discarded by rollback.
        outcomes.complete(5, false);
        outcomes.complete(5, true);
        outcomes.complete(7, true);
        for id in [1, 2, 6, 7] {
            assert_eq!(outcomes.result(id), Some(true));
        }
        for id in [3, 4, 5] {
            assert_eq!(outcomes.result(id), Some(false));
        }
        assert_eq!(outcomes.result(8), None);
    }

    #[test]
    fn persistence_terminal_ranges_wrap_and_eviction_fails_closed() {
        let mut outcomes = PersistOutcomes {
            completed: Some(u32::MAX - 1),
            ..Default::default()
        };
        outcomes.complete(0, false);
        outcomes.complete(1, true);
        assert_eq!(outcomes.result(u32::MAX), Some(false));
        assert_eq!(outcomes.result(0), Some(false));
        assert_eq!(outcomes.result(1), Some(true));
        for id in 2..12 {
            outcomes.complete(id, true);
        }
        assert_eq!(outcomes.result(0), Some(false));
        assert_eq!(outcomes.result(1), Some(false));
        assert_eq!(outcomes.result(11), Some(true));
        assert_eq!(outcomes.result(12), None);
    }

    #[test]
    fn empty_slot_activation_requires_enabled_radio_and_starts_pairing() {
        let mut state = state();
        assert!(state.reduce(&BleCommand::ActivateSlot(1)));
        assert_eq!(state.active_profile, None);
        assert!(state.reduce(&BleCommand::SetBluetoothEnabled(true)));
        assert!(state.reduce(&BleCommand::ActivateSlot(1)));
        assert_eq!(state.active_profile, Some(1));
        assert!(state.pairing_mode);
        assert_eq!(state.link_state, BleLinkState::Pairing);
    }

    #[test]
    fn bluetooth_off_and_factory_reset_clear_runtime_selection() {
        let mut state = state();
        state.reduce(&BleCommand::SetBluetoothEnabled(true));
        state.reduce(&BleCommand::ActivateSlot(2));
        assert!(state.reduce(&BleCommand::CancelPairing));
        assert!(!state.bluetooth_enabled);
        assert_eq!(state.active_profile, None);
        assert_eq!(state.link_state, BleLinkState::BluetoothOff);
        assert!(state.reduce(&BleCommand::FactoryReset));
        assert_eq!(state.device_name.as_str(), "Pager");
        assert!(state.bonds.iter().all(Option::is_none));
    }

    #[test]
    fn invalid_mutations_and_hid_jobs_are_not_control_transitions() {
        let mut state = state();
        let name = heapless::String::try_from("phone").unwrap();
        assert!(!state.reduce(&BleCommand::SetSlotName(3, name.clone())));
        assert!(!state.reduce(&BleCommand::SetSlotName(0, name)));
        assert!(!state.reduce(&BleCommand::TypeString(
            heapless::String::try_from("x").unwrap()
        )));
    }

    #[test]
    fn missing_bonded_hid_cccd_is_healed_once() {
        assert_eq!(bonded_hid_cccd_flags(true, 0), 0b11);
        assert_eq!(bonded_hid_cccd_flags(true, 0b01), 0b01);
        assert_eq!(bonded_hid_cccd_flags(false, 0), 0);
        assert_eq!(bonded_hid_cccd_flags(true, 0b100), 0b111);
    }
}

pub fn store_unique_bond(state: &mut KeyboardState, slot: usize, bond: BondInformation) {
    let same_identity = state.bonds[slot]
        .as_ref()
        .is_some_and(|existing| existing.identity.match_identity(&bond.identity));
    for other in 0..state.bonds.len() {
        if other != slot
            && state.bonds[other]
                .as_ref()
                .is_some_and(|existing| existing.identity.match_identity(&bond.identity))
        {
            state.bonds[other] = None;
            state.cccd_flags[other] = 0;
            state.slot_names[other].clear();
        }
    }
    if !same_identity || state.slot_names[slot].is_empty() {
        state.slot_names[slot] = format_default_peer_name(bond.identity.addr.addr.raw());
    }
    state.bonds[slot] = Some(bond);
}

pub fn remove_duplicate_bonds(state: &mut KeyboardState) -> bool {
    let mut changed = false;
    for slot in 0..state.bonds.len() {
        let Some(identity) = state.bonds[slot].as_ref().map(|bond| bond.identity) else {
            continue;
        };
        for duplicate in slot + 1..state.bonds.len() {
            if state.bonds[duplicate]
                .as_ref()
                .is_some_and(|bond| bond.identity.match_identity(&identity))
            {
                state.bonds[duplicate] = None;
                state.cccd_flags[duplicate] = 0;
                state.slot_names[duplicate].clear();
                changed = true;
            }
        }
    }
    changed
}

/// Session events carry the captured slot so stale events cannot attach a
/// connection to a newly selected profile. Bond data is never formatted.
#[allow(clippy::large_enum_variant)]
pub enum SessionEvent<'a> {
    ControllerStopped,
    Control(&'a BleCommand),
    Connecting(usize),
    Secured {
        slot: usize,
        bond: Option<BondInformation>,
    },
    PairingFailed(usize),
    Disconnected(usize),
    HidReady {
        slot: usize,
        ready: bool,
    },
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub changed: bool,
    pub persist: bool,
    pub disconnect: bool,
}
impl KeyboardState {
    pub fn session_event(&mut self, event: SessionEvent<'_>) -> Effects {
        if matches!(event, SessionEvent::ControllerStopped) {
            self.connected_profile = None;
            self.hid_ready = false;
            self.link_state = if !self.bluetooth_enabled {
                BleLinkState::BluetoothOff
            } else if self.active_profile.is_none() {
                BleLinkState::Idle
            } else if self.pairing_mode {
                BleLinkState::Pairing
            } else {
                BleLinkState::Advertising
            };
            return Effects {
                changed: true,
                ..Effects::default()
            };
        }
        if let SessionEvent::Control(command) = event {
            if self.is_noop(command) {
                return Effects::default();
            }
            let disconnect = self.connected_profile.is_some()
                && !matches!(command, BleCommand::SetSlotName(_, _))
                && !matches!(command, BleCommand::ClearSlot(slot) if self.active_profile != Some(*slot));
            let handled = self.reduce(command);
            return Effects {
                changed: handled,
                persist: handled,
                disconnect: handled && disconnect,
            };
        }
        let slot = match &event {
            SessionEvent::Control(_) | SessionEvent::ControllerStopped => unreachable!(),
            SessionEvent::Connecting(s)
            | SessionEvent::PairingFailed(s)
            | SessionEvent::Disconnected(s) => *s,
            SessionEvent::Secured { slot, .. } | SessionEvent::HidReady { slot, .. } => *slot,
        };
        if slot >= self.bonds.len() || !self.bluetooth_enabled || self.active_profile != Some(slot)
        {
            return Effects {
                disconnect: matches!(
                    event,
                    SessionEvent::Connecting(_) | SessionEvent::Secured { .. }
                ),
                ..Effects::default()
            };
        }
        match event {
            SessionEvent::Control(_) | SessionEvent::ControllerStopped => unreachable!(),
            SessionEvent::Connecting(_) => {
                self.connected_profile = None;
                self.hid_ready = false;
                self.link_state = BleLinkState::Connecting;
                Effects {
                    changed: true,
                    ..Effects::default()
                }
            }
            SessionEvent::Secured { bond, .. } => {
                if let Some(bond) = bond {
                    store_unique_bond(self, slot, bond);
                }
                self.pairing_mode = false;
                self.connected_profile = Some(slot);
                self.link_state = BleLinkState::Connected;
                Effects {
                    changed: true,
                    persist: true,
                    disconnect: false,
                }
            }
            SessionEvent::PairingFailed(_) => {
                self.connected_profile = None;
                self.hid_ready = false;
                self.link_state = BleLinkState::Disconnecting;
                Effects {
                    changed: true,
                    disconnect: true,
                    persist: false,
                }
            }
            SessionEvent::Disconnected(_) => {
                self.connected_profile = None;
                self.hid_ready = false;
                self.link_state = if self.pairing_mode {
                    BleLinkState::Pairing
                } else {
                    BleLinkState::Advertising
                };
                Effects {
                    changed: true,
                    ..Effects::default()
                }
            }
            SessionEvent::HidReady { ready, .. } => {
                let ready = ready
                    && self.connected_profile == Some(slot)
                    && self.link_state == BleLinkState::Connected;
                let changed = self.hid_ready != ready;
                self.hid_ready = ready;
                Effects {
                    changed,
                    ..Effects::default()
                }
            }
        }
    }
}

pub fn cccd_flags(values: [Option<&[u8]>; 3]) -> u8 {
    values.into_iter().enumerate().fold(0, |flags, (i, value)| {
        flags
            | if value.is_some_and(|v| v.len() >= 2 && u16::from_le_bytes([v[0], v[1]]) != 0) {
                1 << i
            } else {
                0
            }
    })
}

#[cfg(test)]
mod session_tests {
    use super::*;
    fn state() -> KeyboardState {
        KeyboardState {
            bonds: [None, None, None],
            cccd_flags: [0; 3],
            slot_names: Default::default(),
            device_name: heapless::String::new(),
            active_profile: Some(0),
            connected_profile: None,
            bluetooth_enabled: true,
            link_state: BleLinkState::Pairing,
            pairing_mode: true,
            fast_advertising: false,
            hid_ready: false,
        }
    }
    #[test]
    fn security_gates_hid_and_failure_clears_readiness() {
        let mut s = state();
        s.session_event(SessionEvent::Connecting(0));
        s.session_event(SessionEvent::HidReady {
            slot: 0,
            ready: true,
        });
        assert!(!s.hid_ready);
        assert!(
            s.session_event(SessionEvent::Secured {
                slot: 0,
                bond: None
            })
            .persist
        );
        s.session_event(SessionEvent::HidReady {
            slot: 0,
            ready: true,
        });
        assert!(s.hid_ready);
        assert!(s.session_event(SessionEvent::PairingFailed(0)).disconnect);
        assert!(!s.hid_ready);
        assert_eq!(s.connected_profile, None);
        s.session_event(SessionEvent::Disconnected(0));
        assert_eq!(s.link_state, BleLinkState::Advertising);
    }
    #[test]
    fn stale_slot_and_off_events_cannot_reconnect_or_mark_ready() {
        let mut s = state();
        assert!(
            s.session_event(SessionEvent::Secured {
                slot: 1,
                bond: None
            })
            .disconnect
        );
        assert_eq!(s.active_profile, Some(0));
        assert_eq!(s.connected_profile, None);
        s.reduce(&BleCommand::SetBluetoothEnabled(false));
        assert!(s.session_event(SessionEvent::Connecting(0)).disconnect);
        s.session_event(SessionEvent::HidReady {
            slot: 0,
            ready: true,
        });
        assert!(!s.hid_ready);
        assert_eq!(s.link_state, BleLinkState::BluetoothOff);
    }
    #[test]
    fn control_and_session_events_share_disconnect_and_noop_effects() {
        let mut s = state();
        s.session_event(SessionEvent::Secured {
            slot: 0,
            bond: None,
        });
        s.session_event(SessionEvent::HidReady {
            slot: 0,
            ready: true,
        });
        let effects = s.session_event(SessionEvent::Control(&BleCommand::SetBluetoothEnabled(
            false,
        )));
        assert!(effects.persist && effects.disconnect && effects.changed);
        assert_eq!(s.link_state, BleLinkState::BluetoothOff);
        assert!(!s.hid_ready);
        assert_eq!(
            s.session_event(SessionEvent::Control(&BleCommand::SetBluetoothEnabled(
                false
            ))),
            Effects::default()
        );
    }
    #[test]
    fn type_result_survives_concurrent_control_completion_and_stale_finish() {
        assert!(register_type_control(101));
        assert!(!register_type_control(102));
        finish_type_control(100);
        assert!(!register_type_control(102));
        complete_control(101, 3);
        complete_control(102, 0);
        assert_eq!(TYPE_CONTROL_RESULT.try_take(), Some((101, 3)));
        finish_type_control(101);
        assert!(register_type_control(103));
        finish_type_control(103);
    }
    #[test]
    fn controller_stop_clears_transient_link_and_preserves_durable_selection() {
        let mut s = state();
        s.session_event(SessionEvent::Secured {
            slot: 0,
            bond: None,
        });
        s.session_event(SessionEvent::HidReady {
            slot: 0,
            ready: true,
        });
        s.cccd_flags = [3, 1, 0];
        s.session_event(SessionEvent::ControllerStopped);
        assert_eq!(s.connected_profile, None);
        assert!(!s.hid_ready);
        assert_eq!(s.active_profile, Some(0));
        assert_eq!(s.cccd_flags, [3, 1, 0]);
        assert_eq!(s.link_state, BleLinkState::Advertising);
        s.reduce(&BleCommand::SetBluetoothEnabled(false));
        s.session_event(SessionEvent::ControllerStopped);
        assert_eq!(s.link_state, BleLinkState::BluetoothOff);
    }
    #[test]
    fn cccd_handles_missing_short_and_indication_values() {
        assert_eq!(cccd_flags([None, Some(&[1]), Some(&[2, 0])]), 4);
        assert_eq!(cccd_flags([Some(&[1, 0]), Some(&[0, 0]), Some(&[0, 1])]), 5);
    }
}
