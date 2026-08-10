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

pub static BLE_COMMANDS: Channel<ThreadModeRawMutex, BleCommand, 8> = Channel::new();
pub static PERSIST_STATE: Signal<ThreadModeRawMutex, u32> = Signal::new();
pub static PERSIST_DONE: Signal<ThreadModeRawMutex, (u32, bool)> = Signal::new();
static PERSIST_SEQUENCE: AtomicU32 = AtomicU32::new(0);
pub static BLE_CONTROL_RESULT: Signal<ThreadModeRawMutex, u8> = Signal::new();
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

pub fn try_send_command(command: BleCommand) -> bool {
    if BLE_COMMANDS.try_send(command).is_ok() {
        true
    } else {
        DROPPED_COMMANDS.fetch_add(1, Ordering::Relaxed);
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
