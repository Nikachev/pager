#![allow(dead_code)]
use core::cell::RefCell;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use embassy_sync::channel::Channel;
use trouble_host::prelude::*;

// ---------------------------------------------------------------------------
// GATT Server and Services with Security & Encryption (trouble-host)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[gatt_server(attribute_table_size = 64)]
pub struct Server {
    #[allow(dead_code)]
    pub custom_service: CustomService,
    #[allow(dead_code)]
    pub hid_service: HidService,
    #[allow(dead_code)]
    pub battery_service: BatteryService,
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
    #[characteristic(uuid = "2a4a", read, value = [0x11, 0x01, 0x00, 0x03], permissions(encrypted))]
    pub hid_info: [u8; 4],

    #[characteristic(uuid = "2a4b", read, value = HID_REPORT_DESCRIPTOR, permissions(encrypted))]
    pub report_map: [u8; 67],

    #[characteristic(uuid = "2a4c", write_without_response, permissions(encrypted))]
    pub hid_control_point: u8,

    #[characteristic(uuid = "2a4e", read, write_without_response, value = 1, permissions(encrypted))]
    pub protocol_mode: u8,

    #[descriptor(uuid = "2908", read = encrypted, value = [0u8, 1u8])]
    #[characteristic(uuid = "2a4d", read, notify, permissions(encrypted))]
    pub input_keyboard: [u8; 8],

    #[descriptor(uuid = "2908", read = encrypted, value = [0u8, 2u8])]
    #[characteristic(uuid = "2a4d", read, write, write_without_response, permissions(encrypted))]
    pub output_keyboard: [u8; 1],
}

#[gatt_service(uuid = "180f")]
pub struct BatteryService {
    #[characteristic(uuid = "2a19", read, notify, value = 100, permissions(encrypted))]
    pub level: u8,
}

pub struct KeyboardState {
    pub bonds: [Option<()>; 3],
    pub active_slot: usize,
    pub pairing_mode: bool,
}

pub static KEYBOARD_STATE: SyncMutex<ThreadModeRawMutex, RefCell<KeyboardState>> =
    SyncMutex::new(RefCell::new(KeyboardState {
        bonds: [None, None, None],
        active_slot: 0,
        pairing_mode: false,
    }));

pub async fn erase_bond_slot<F: embedded_storage_async::nor_flash::NorFlash>(flash: &mut F, _slot: usize) {
    let _ = flash.erase(0x000FC000, 0x00100000).await;
}

#[allow(dead_code)]
pub enum BleCommand {
    Disconnect,
    RestartAdvertising,
    TypeString(heapless::String<128>),
}

pub static BLE_COMMANDS: Channel<ThreadModeRawMutex, BleCommand, 8> = Channel::new();

#[allow(dead_code)]
pub fn ascii_to_hid(c: char) -> Option<(u8, u8)> {
    let mut modifiers = 0;
    let keycode = match c {
        'a'..='z' => (c as u8 - b'a') + 0x04,
        'A'..='Z' => {
            modifiers = 0x02; // Left Shift
            (c as u8 - b'A') + 0x04
        }
        '1'..='9' => (c as u8 - b'1') + 0x1E,
        '0' => 0x27,
        '\n' | '\r' => 0x28, // Enter
        ' ' => 0x2C,         // Space
        '!' => { modifiers = 0x02; 0x1E },
        '@' => { modifiers = 0x02; 0x1F },
        '#' => { modifiers = 0x02; 0x20 },
        '$' => { modifiers = 0x02; 0x21 },
        '%' => { modifiers = 0x02; 0x22 },
        '^' => { modifiers = 0x02; 0x23 },
        '&' => { modifiers = 0x02; 0x24 },
        '*' => { modifiers = 0x02; 0x25 },
        '(' => { modifiers = 0x02; 0x26 },
        ')' => { modifiers = 0x02; 0x27 },
        '-' => 0x2D,
        '_' => { modifiers = 0x02; 0x2D },
        '=' => 0x2E,
        '+' => { modifiers = 0x02; 0x2E },
        '[' => 0x2F,
        '{' => { modifiers = 0x02; 0x2F },
        ']' => 0x30,
        '}' => { modifiers = 0x02; 0x30 },
        '\\' => 0x31,
        '|' => { modifiers = 0x02; 0x31 },
        ';' => 0x33,
        ':' => { modifiers = 0x02; 0x33 },
        '\'' => 0x34,
        '"' => { modifiers = 0x02; 0x34 },
        '`' => 0x35,
        '~' => { modifiers = 0x02; 0x35 },
        ',' => 0x36,
        '<' => { modifiers = 0x02; 0x36 },
        '.' => 0x37,
        '>' => { modifiers = 0x02; 0x37 },
        '/' => 0x38,
        '?' => { modifiers = 0x02; 0x38 },
        _ => return None,
    };
    Some((modifiers, keycode))
}
