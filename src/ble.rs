#![allow(dead_code)]

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use trouble_host::prelude::*;

// ---------------------------------------------------------------------------
// GATT Server and Services with Security & Encryption (trouble-host)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[gatt_server(attribute_table_size = 128)]
pub struct Server {
    #[allow(dead_code)]
    pub custom_service: CustomService,
    #[allow(dead_code)]
    pub hid_service: HidService,
    #[allow(dead_code)]
    pub battery_service: BatteryService,
    #[allow(dead_code)]
    pub device_information_service: DeviceInformationService,
}

#[gatt_service(uuid = "9e7a0001-0b3e-46e8-ad30-7746bad7128a")]
pub struct CustomService {
    #[characteristic(uuid = "9e7a0002-0b3e-46e8-ad30-7746bad7128a", write)]
    pub led: u8,

    #[characteristic(uuid = "9e7a0003-0b3e-46e8-ad30-7746bad7128a", read, notify)]
    pub status: u8,
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
    // TODO(hardware): replace with ADC-based VBAT measurement once the battery
    // divider is wired. Until then expose a deliberately obvious placeholder.
    #[characteristic(uuid = "2a19", read, notify, value = 13, permissions(encrypted))]
    pub level: u8,
}

#[gatt_service(uuid = "180a")]
pub struct DeviceInformationService {
    #[characteristic(uuid = "2a29", read, value = *b"Antigravity")]
    pub manufacturer_name: [u8; 11],

    #[characteristic(uuid = "2a24", read, value = *b"nice_nano_v2")]
    pub model_number: [u8; 12],
}

pub struct KeyboardState {
    pub bonds: [Option<BondInformation>; 3],
    pub active_slot: usize,
    pub pairing_mode: bool,
}

pub static KEYBOARD_STATE: SyncMutex<ThreadModeRawMutex, RefCell<KeyboardState>> =
    SyncMutex::new(RefCell::new(KeyboardState {
        bonds: [None, None, None],
        active_slot: 0,
        pairing_mode: false,
    }));

#[allow(dead_code)]
pub enum BleCommand {
    SyncActiveBond,
    Disconnect,
    RestartAdvertising,
    TypeString(heapless::String<128>),
}

pub static BLE_COMMANDS: Channel<ThreadModeRawMutex, BleCommand, 8> = Channel::new();
pub static PERSIST_STATE: Signal<ThreadModeRawMutex, ()> = Signal::new();
pub static DROPPED_COMMANDS: AtomicU32 = AtomicU32::new(0);

pub fn try_send_command(command: BleCommand) -> bool {
    if BLE_COMMANDS.try_send(command).is_ok() {
        true
    } else {
        DROPPED_COMMANDS.fetch_add(1, Ordering::Relaxed);
        false
    }
}

#[allow(dead_code)]
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
