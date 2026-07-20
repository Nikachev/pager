use defmt::*;
use nrf_softdevice::ble::advertisement_builder::{
    Flag, LegacyAdvertisementBuilder, ServiceList, ServiceUuid16,
};
use nrf_softdevice::ble::{
    gatt_server, peripheral, Connection, Uuid, EncryptionInfo, IdentityKey, MasterId, HciStatus
};
use nrf_softdevice::Softdevice;
use nrf_softdevice::ble::security::{IoCapabilities, SecurityHandler};
use nrf_softdevice::ble::SecurityMode;
use nrf_softdevice::ble::gatt_server::builder::ServiceBuilder;
use nrf_softdevice::ble::gatt_server::characteristic::{Attribute, Metadata, Properties};
use nrf_softdevice::ble::gatt_server::{RegisterError, Service};
use embedded_storage_async::nor_flash::NorFlash as _;
use embedded_storage_async::nor_flash::ReadNorFlash as _;
use static_cell::StaticCell;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::Mutex as SyncMutex;
use core::cell::RefCell;
use embassy_futures::select::{select, Either};

// Custom 128-bit Base UUID: 9e7a0000-0b3e-46e8-ad30-7746bad7128a
// Service UUID: 9e7a0001-0b3e-46e8-ad30-7746bad7128a
// LED Characteristic UUID: 9e7a0002-0b3e-46e8-ad30-7746bad7128a
// Status Characteristic UUID: 9e7a0003-0b3e-46e8-ad30-7746bad7128a

#[nrf_softdevice::gatt_service(uuid = "9e7a0001-0b3e-46e8-ad30-7746bad7128a")]
pub struct CustomService {
    #[characteristic(uuid = "9e7a0002-0b3e-46e8-ad30-7746bad7128a", write)]
    pub led: u8,

    #[characteristic(uuid = "9e7a0003-0b3e-46e8-ad30-7746bad7128a", read, notify)]
    pub status: u8,
}

// HID Keyboard setup
macro_rules! count {
    () => { 0u8 };
    ($x:tt $($xs:tt)*) => {1u8 + count!($($xs)*)};
}

macro_rules! hid {
    ($(( $($xs:tt),*)),+ $(,)?) => { &[ $( (count!($($xs)*)-1) | $($xs),* ),* ] };
}

// Main items
pub const HIDINPUT: u8 = 0x80;
pub const HIDOUTPUT: u8 = 0x90;
pub const COLLECTION: u8 = 0xa0;
pub const END_COLLECTION: u8 = 0xc0;

// Global items
pub const USAGE_PAGE: u8 = 0x04;
pub const LOGICAL_MINIMUM: u8 = 0x14;
pub const LOGICAL_MAXIMUM: u8 = 0x24;
pub const REPORT_SIZE: u8 = 0x74;
pub const REPORT_ID: u8 = 0x84;
pub const REPORT_COUNT: u8 = 0x94;

// Local items
pub const USAGE: u8 = 0x08;
pub const USAGE_MINIMUM: u8 = 0x18;
pub const USAGE_MAXIMUM: u8 = 0x28;

const HID_REPORT_DESCRIPTOR: &[u8] = hid!(
    (USAGE_PAGE, 0x01), // USAGE_PAGE (Generic Desktop Ctrls)
    (USAGE, 0x06),      // USAGE (Keyboard)
    (COLLECTION, 0x01), // COLLECTION (Application)
    // ------------------------------------------------- Keyboard
    (REPORT_ID, 0x01),        //   REPORT_ID (1)
    (USAGE_PAGE, 0x07),       //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0xE0),    //   USAGE_MINIMUM (0xE0)
    (USAGE_MAXIMUM, 0xE7),    //   USAGE_MAXIMUM (0xE7)
    (LOGICAL_MINIMUM, 0x00),  //   LOGICAL_MINIMUM (0)
    (LOGICAL_MAXIMUM, 0x01),  //   Logical Maximum (1)
    (REPORT_SIZE, 0x01),      //   REPORT_SIZE (1)
    (REPORT_COUNT, 0x08),     //   REPORT_COUNT (8)
    (HIDINPUT, 0x02),         //   INPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x01),     //   REPORT_COUNT (1) ; 1 byte (Reserved)
    (REPORT_SIZE, 0x08),      //   REPORT_SIZE (8)
    (HIDINPUT, 0x01),         //   INPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (REPORT_COUNT, 0x05),     //   REPORT_COUNT (5) ; 5 bits (Num lock, Caps lock, Scroll lock, Compose, Kana)
    (REPORT_SIZE, 0x01),      //   REPORT_SIZE (1)
    (USAGE_PAGE, 0x08),       //   USAGE_PAGE (LEDs)
    (USAGE_MINIMUM, 0x01),    //   USAGE_MINIMUM (0x01) ; Num Lock
    (USAGE_MAXIMUM, 0x05),    //   USAGE_MAXIMUM (0x05) ; Kana
    (HIDOUTPUT, 0x02),        //   OUTPUT (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x01),     //   REPORT_COUNT (1) ; 3 bits (Padding)
    (REPORT_SIZE, 0x03),      //   REPORT_SIZE (3)
    (HIDOUTPUT, 0x01),        //   OUTPUT (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    (REPORT_COUNT, 0x06),     //   REPORT_COUNT (6) ; 6 bytes (Keys)
    (REPORT_SIZE, 0x08),      //   REPORT_SIZE(8)
    (LOGICAL_MINIMUM, 0x00),  //   LOGICAL_MINIMUM(0)
    (LOGICAL_MAXIMUM, 0x65),  //   LOGICAL_MAXIMUM(0x65) ; 101 keys
    (USAGE_PAGE, 0x07),       //   USAGE_PAGE (Kbrd/Keypad)
    (USAGE_MINIMUM, 0x00),    //   USAGE_MINIMUM (0)
    (USAGE_MAXIMUM, 0x65),    //   USAGE_MAXIMUM (0x65)
    (HIDINPUT, 0x00),         //   INPUT (Data,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    (END_COLLECTION),         // END_COLLECTION
);

pub struct HidService {
    pub input_report_value_handle: u16,
    pub input_report_cccd_handle: u16,
    pub boot_input_report_value_handle: u16,
    pub boot_input_report_cccd_handle: u16,
    pub protocol_mode_value_handle: u16,
}

// HID protocol mode negotiated by the host: 0 = Boot Protocol, 1 = Report Protocol.
// macOS negotiates this via the HID Protocol Mode characteristic (0x2A4E); without
// it the host may not route the keyboard's input reports as keystrokes.
pub static PROTOCOL_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);

impl HidService {
    pub fn new(sd: &mut Softdevice) -> Result<Self, RegisterError> {
        let mut sb = ServiceBuilder::new(sd, Uuid::new_16(0x1812))?;

        // 1. Report Map (0x2A4B)
        sb.add_characteristic(
            Uuid::new_16(0x2a4b),
            Attribute::new(HID_REPORT_DESCRIPTOR),
            Metadata::new(Properties::new().read()),
        )?.build();

        // 2. HID Info (0x2A4a)
        sb.add_characteristic(
            Uuid::new_16(0x2a4a),
            Attribute::new(&[0x11, 0x01, 0x00, 0x03]),
            Metadata::new(Properties::new().read()),
        )?.build();

        // 3. HID Control Point (0x2A4C) - suspend/resume (write only)
        sb.add_characteristic(
            Uuid::new_16(0x2a4c),
            Attribute::new(&[0u8; 1]),
            Metadata::new(Properties::new().write()),
        )?.build();

        // 4. HID Protocol Mode (0x2A4E) - read/write, default Report Protocol
        let pm = sb.add_characteristic(
            Uuid::new_16(0x2a4e),
            Attribute::new(&[1u8]),
            Metadata::new(Properties::new().read().write()),
        )?;
        let protocol_mode_value_handle = pm.build().value_handle;

        // 5. Input Report (0x2A4D) - Report Protocol (Report ID 0x01)
        let mut char_b = sb.add_characteristic(
            Uuid::new_16(0x2a4d),
            Attribute::new(&[0u8; 8]).security(SecurityMode::JustWorks),
            Metadata::new(Properties::new().read().write().notify()).security(SecurityMode::JustWorks),
        )?;
        char_b.add_descriptor(
            Uuid::new_16(0x2908),
            Attribute::new(&[1u8, 1u8]),
        )?;
        let handles = char_b.build();

        // 6. Boot Keyboard Input Report (0x2A22) - Boot Protocol (8 bytes, no Report ID)
        let mut boot_b = sb.add_characteristic(
            Uuid::new_16(0x2a22),
            Attribute::new(&[0u8; 8]).security(SecurityMode::JustWorks),
            Metadata::new(Properties::new().read().write().notify()).security(SecurityMode::JustWorks),
        )?;
        boot_b.add_descriptor(
            Uuid::new_16(0x2908),
            Attribute::new(&[0u8, 1u8]),
        )?;
        let boot_handles = boot_b.build();

        sb.build();

        Ok(Self {
            input_report_value_handle: handles.value_handle,
            input_report_cccd_handle: handles.cccd_handle,
            boot_input_report_value_handle: boot_handles.value_handle,
            boot_input_report_cccd_handle: boot_handles.cccd_handle,
            protocol_mode_value_handle,
        })
    }

    // Send a single keystroke report. The report is the 8-byte keyboard report
    // (modifier, reserved, 6 keycodes). Over HID-over-GATT the Report Reference
    // descriptor on the Input Report characteristic already identifies the
    // Report ID, so the characteristic VALUE must be the raw report data with NO
    // Report ID prefix (matching the nRF HID keyboard reference). This holds for
    // both Report Protocol (0x2A4D) and Boot Protocol (0x2A22).
    pub fn send_key(&self, conn: &Connection, report: &[u8; 8]) -> Result<(), gatt_server::NotifyValueError> {
        let mode = PROTOCOL_MODE.load(core::sync::atomic::Ordering::Relaxed);
        if mode == 0 {
            gatt_server::notify_value(conn, self.boot_input_report_value_handle, report)
        } else {
            gatt_server::notify_value(conn, self.input_report_value_handle, report)
        }
    }
}

pub struct Server {
    pub custom: CustomService,
    pub hid: HidService,
}

impl Server {
    pub fn new(sd: &mut Softdevice) -> Result<Self, RegisterError> {
        let custom = CustomService::new(sd)?;
        let hid = HidService::new(sd)?;
        Ok(Self { custom, hid })
    }
}

pub enum ServerEvent {
    Custom(CustomServiceEvent),
    Hid(HidServiceEvent),
}

pub enum HidServiceEvent {
    InputReportCccdWrite { notifications: bool },
}

impl gatt_server::Server for Server {
    type Event = ServerEvent;

    fn on_write(
        &self,
        conn: &Connection,
        handle: u16,
        op: gatt_server::WriteOp,
        offset: usize,
        data: &[u8],
    ) -> Option<Self::Event> {
        if let Some(event) = self.custom.on_write(handle, data) {
            return Some(ServerEvent::Custom(event));
        }
        if handle == self.hid.input_report_cccd_handle && !data.is_empty() {
            let notifications = (data[0] & 0x01) != 0;
            return Some(ServerEvent::Hid(HidServiceEvent::InputReportCccdWrite { notifications }));
        }
        if handle == self.hid.boot_input_report_cccd_handle && !data.is_empty() {
            // Boot Keyboard Input Report notifications toggled; accepted.
            return None;
        }
        if handle == self.hid.protocol_mode_value_handle && !data.is_empty() {
            PROTOCOL_MODE.store(data[0], core::sync::atomic::Ordering::Relaxed);
            crate::log_msg!("BLE: HID protocol mode set to {}", data[0]);
            return None;
        }
        // HID Control Point (suspend/resume) and any other writes are accepted.
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BondInfo {
    pub master_id: MasterId,
    pub key: EncryptionInfo,
    pub peer_id: IdentityKey,
}

pub struct KeyboardState {
    pub bonds: [Option<BondInfo>; 3],
    pub active_slot: usize,
    pub pairing_mode: bool,
}

pub static KEYBOARD_STATE: SyncMutex<ThreadModeRawMutex, RefCell<KeyboardState>> =
    SyncMutex::new(RefCell::new(KeyboardState {
        bonds: [None, None, None],
        active_slot: 0,
        pairing_mode: false,
    }));

pub enum BleCommand {
    Disconnect,
    RestartAdvertising,
    TypeString(heapless::String<128>),
}

pub static BLE_COMMANDS: Channel<ThreadModeRawMutex, BleCommand, 8> = Channel::new();

// ---------------------------------------------------------------------------
// Persistent BLE bonding storage
//
// Bonding keys are kept in RAM by the SoftDevice, but that is lost on every
// reboot/OTA. macOS (and other hosts) remember their side of the bond, so
// after a reboot the host fails with "Peer removed pairing information".
// We mirror the 3 bond slots into a dedicated flash page so they survive
// reboots and stay in sync with the host's stored bond.
// ---------------------------------------------------------------------------

const BONDS_MAGIC: u32 = 0xBEAC_15A1;
// 4-byte magic + 3 slots * (1 present flag + 10 + 17 + 23 bytes) = 4 + 3*51 = 157 bytes
const BOND_SLOT_SIZE: usize = 1 + 10 + 17 + 23;

fn bond_slot_offset(slot: usize) -> usize {
    4 + slot * BOND_SLOT_SIZE
}

fn write_bond_into(buf: &mut [u8], slot: usize, bond: &BondInfo) {
    let base = bond_slot_offset(slot);
    buf[base] = 1;
    let mut o = base + 1;
    buf[o..o + 2].copy_from_slice(&bond.master_id.ediv.to_le_bytes());
    o += 2;
    buf[o..o + 8].copy_from_slice(&bond.master_id.rand);
    o += 8;
    buf[o..o + 16].copy_from_slice(&bond.key.ltk);
    o += 16;
    buf[o..o + 1].copy_from_slice(&[bond.key.flags]);
    o += 1;
    let raw_peer = bond.peer_id.as_raw();
    buf[o..o + 16].copy_from_slice(&raw_peer.id_info.irk);
    o += 16;
    let addr = bond.peer_id.addr;
    buf[o..o + 1].copy_from_slice(&[addr.flags]);
    o += 1;
    buf[o..o + 6].copy_from_slice(&addr.bytes);
}

fn read_bond_from(buf: &[u8], slot: usize) -> Option<BondInfo> {
    let base = bond_slot_offset(slot);
    if buf[base] != 1 {
        return None;
    }
    let mut o = base + 1;
    let ediv = u16::from_le_bytes([buf[o], buf[o + 1]]);
    o += 2;
    let mut rand = [0u8; 8];
    rand.copy_from_slice(&buf[o..o + 8]);
    o += 8;
    let mut ltk = [0u8; 16];
    ltk.copy_from_slice(&buf[o..o + 16]);
    o += 16;
    let flags = buf[o];
    o += 1;
    let mut irk = [0u8; 16];
    irk.copy_from_slice(&buf[o..o + 16]);
    o += 16;
    let addr_flags = buf[o];
    o += 1;
    let mut addr_bytes = [0u8; 6];
    addr_bytes.copy_from_slice(&buf[o..o + 6]);

    let addr = nrf_softdevice::ble::Address::new(
        unsafe { core::mem::transmute((addr_flags >> 1) as u8) },
        addr_bytes,
    );

    // Rebuild the IdentityKey from raw bytes (irk + address) to avoid
    // depending on the private IRK type layout.
    let id_key = nrf_softdevice::raw::ble_gap_id_key_t {
        id_info: nrf_softdevice::raw::ble_gap_irk_t { irk },
        id_addr_info: *addr.as_raw(),
    };
    let peer_id = IdentityKey::from_raw(id_key);

    Some(BondInfo {
        master_id: MasterId { ediv, rand },
        key: EncryptionInfo { ltk, flags },
        peer_id,
    })
}

/// Persist the current in-RAM bond slots to flash.
pub async fn save_bonds(
    flash: &mut nrf_softdevice::Flash,
    bonds: &[Option<BondInfo>; 3],
) {
    let mut buf = [0u8; BOND_SLOT_SIZE * 3 + 4];
    buf[0..4].copy_from_slice(&BONDS_MAGIC.to_le_bytes());
    for (i, b) in bonds.iter().enumerate() {
        if let Some(bond) = b {
            write_bond_into(&mut buf, i, bond);
        } else {
            buf[bond_slot_offset(i)] = 0;
        }
    }

    if let Err(e) = flash.erase(crate::web::BONDS_STORAGE_ADDR, crate::web::BONDS_STORAGE_ADDR + 4096).await {
        warn!("Failed to erase bonds page: {:?}", e);
        return;
    }
    if let Err(e) = flash.write(crate::web::BONDS_STORAGE_ADDR, &buf).await {
        warn!("Failed to write bonds: {:?}", e);
    }
}

/// Erase a single bond slot in flash (used when a bond is deleted).
pub async fn erase_bond_slot(flash: &mut nrf_softdevice::Flash, slot: usize) {
    let current = load_bonds_flash(flash).await;
    let mut bonds: [Option<BondInfo>; 3] = [None, None, None];
    for i in 0..3 {
        if i != slot {
            bonds[i] = read_bond_from(&current, i);
        }
    }
    save_bonds(flash, &bonds).await;
}

/// Read the raw flash page containing the bonds.
async fn load_bonds_flash(flash: &mut nrf_softdevice::Flash) -> [u8; BOND_SLOT_SIZE * 3 + 4] {
    let mut buf = [0u8; BOND_SLOT_SIZE * 3 + 4];
    let _ = flash.read(crate::web::BONDS_STORAGE_ADDR, &mut buf).await;
    buf
}

/// Restore in-RAM bond slots from flash at startup.
pub async fn restore_bonds_from_flash(flash: &mut nrf_softdevice::Flash) {
    let buf = load_bonds_flash(flash).await;
    if u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) != BONDS_MAGIC {
        return;
    }
    KEYBOARD_STATE.lock(|state| {
        let mut s = state.borrow_mut();
        for i in 0..3 {
            s.bonds[i] = read_bond_from(&buf, i);
        }
    });
}

struct MySecurityHandler;

// Channel used to hand bond snapshots from the (sync) SecurityHandler callback
// to the async persistence task, which owns the flash mutex.
pub static BOND_SAVE_CHANNEL: Channel<ThreadModeRawMutex, [Option<BondInfo>; 3], 2> = Channel::new();

impl SecurityHandler for MySecurityHandler {
    fn io_capabilities(&self) -> IoCapabilities {
        IoCapabilities::None
    }

    fn can_bond(&self, _conn: &Connection) -> bool {
        // Always allow bonding. If the host already has a (possibly stale)
        // bond, allowing (re)bonding lets the SoftDevice transparently
        // re-pair instead of the host failing with "Peer removed pairing
        // information" (CoreBluetooth Code=14).
        true
    }

    fn on_bonded(&self, _conn: &Connection, master_id: MasterId, key: EncryptionInfo, peer_id: IdentityKey) {
        crate::log_msg!("BLE Bonded! Master ID: {:?}, peer: {:?}", master_id, peer_id);
        let mut bonds_snapshot: [Option<BondInfo>; 3] = [None, None, None];
        KEYBOARD_STATE.lock(|state| {
            let mut s = state.borrow_mut();
            let slot = s.active_slot;
            s.bonds[slot] = Some(BondInfo {
                master_id,
                key,
                peer_id,
            });
            s.pairing_mode = false;
            for i in 0..3 {
                bonds_snapshot[i] = s.bonds[i].clone();
            }
        });
        // Hand the snapshot to the async persistence task. We must NOT block
        // here (on_bonded runs inside the GATT server async context), so we
        // only signal via the channel.
        let _ = BOND_SAVE_CHANNEL.try_send(bonds_snapshot);
    }

    fn get_key(&self, _conn: &Connection, master_id: MasterId) -> Option<EncryptionInfo> {
        KEYBOARD_STATE.lock(|state| {
            let s = state.borrow();
            for bond in s.bonds.iter().flatten() {
                if bond.master_id == master_id {
                    crate::log_msg!("BLE: Found matching bond key for master ID: {:?}", master_id);
                    return Some(bond.key);
                }
            }
            None
        })
    }
}

pub fn register_dis_and_bas(sd: &mut Softdevice) -> Result<(), RegisterError> {
    let mut dis_sb = ServiceBuilder::new(sd, Uuid::new_16(0x180a))?;
    dis_sb.add_characteristic(
        Uuid::new_16(0x2a29),
        Attribute::new("Embassy"),
        Metadata::new(Properties::new().read()),
    )?.build();
    dis_sb.add_characteristic(
        Uuid::new_16(0x2a24),
        Attribute::new("nice_nano_v2"),
        Metadata::new(Properties::new().read()),
    )?.build();
    dis_sb.build();

    let mut bas_sb = ServiceBuilder::new(sd, Uuid::new_16(0x180f))?;
    bas_sb.add_characteristic(
        Uuid::new_16(0x2a19),
        Attribute::new(&[100u8]),
        Metadata::new(Properties::new().read().notify()),
    )?.build();
    bas_sb.build();

    Ok(())
}

fn ascii_to_hid(c: char) -> Option<(u8, u8)> {
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

// Async task that persists bond snapshots to flash. It owns the flash mutex so
// the (sync) SecurityHandler callback never has to block on flash I/O.
#[embassy_executor::task]
pub async fn bond_persist_task(
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_softdevice::Flash>,
) -> ! {
    loop {
        let bonds = BOND_SAVE_CHANNEL.receive().await;
        let mut flash = flash_mutex.lock().await;
        save_bonds(&mut flash, &bonds).await;
    }
}

#[embassy_executor::task]
pub async fn ble_task(
    sd: &'static Softdevice,
    server: &'static Server,
    flash_mutex: &'static embassy_sync::mutex::Mutex<ThreadModeRawMutex, nrf_softdevice::Flash>,
) -> ! {
    crate::log_msg!("BLE GATT Server task started.");

    // Restore previously bonded peers from flash so the host's stored bond
    // still matches after a reboot/OTA (prevents "Peer removed pairing information").
    {
        let mut flash = flash_mutex.lock().await;
        restore_bonds_from_flash(&mut flash).await;
    }

    // Derive stable, unique BLE MAC address and name suffix from FICR DEVICEID to bypass host caches without blocking
    let device_id = unsafe { core::ptr::read_volatile(0x100000a4 as *const u32) };
    // Use a fixed RANDOM STATIC BLE address base, with a monotonically
    // increasing boot counter (stored in flash) folded into the address so that
    // every reboot presents macOS with a *new* device identity. This avoids the
    // host replaying a stale bond and failing with "Peer removed pairing
    // information" (CoreBluetooth Code=14). The two most-significant bits are
    // set for a Random Static address.
    let boot_count = {
        let mut flash = flash_mutex.lock().await;
        crate::web::next_boot_count(&mut flash).await
    };
    let base = [0xDAu8, 0x3C, 0xF3, 0x52, 0x35, 0xE6];
    let mut addr_bytes = base;
    // Fold the boot counter into the upper bytes so each boot differs.
    addr_bytes[0] = 0xC0 | ((boot_count & 0x3f) as u8);
    addr_bytes[1] = (boot_count >> 6) as u8;
    addr_bytes[2] = (boot_count >> 14) as u8;
    let addr = nrf_softdevice::ble::Address::new(
        nrf_softdevice::ble::AddressType::RandomStatic,
        addr_bytes,
    );
    nrf_softdevice::ble::set_address(sd, &addr);

    let mut name_buf = *b"nice_nano_XXXX";
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    name_buf[10] = HEX_CHARS[((device_id >> 12) & 0xf) as usize];
    name_buf[11] = HEX_CHARS[((device_id >> 8) & 0xf) as usize];
    name_buf[12] = HEX_CHARS[((device_id >> 4) & 0xf) as usize];
    name_buf[13] = HEX_CHARS[(device_id & 0xf) as usize];

    unsafe {
        let sec_mode = SecurityMode::Open.into_raw();
        let ret = nrf_softdevice::raw::sd_ble_gap_device_name_set(&sec_mode, name_buf.as_ptr(), name_buf.len() as u16);
        if ret != 0 {
            warn!("Failed to set GAP device name: {}", ret);
        }
    }

    let name_str = core::str::from_utf8(&name_buf).unwrap_or("nice_nano");
    let scan_payload = LegacyAdvertisementBuilder::new()
        .short_name(name_str)
        .build();

    static SEC_HANDLER: StaticCell<MySecurityHandler> = StaticCell::new();
    let sec_handler = &*SEC_HANDLER.init(MySecurityHandler);
        let sec_handler_ref: &MySecurityHandler = sec_handler;

        loop {
        let in_pairing_mode = KEYBOARD_STATE.lock(|state| {
            state.borrow().pairing_mode
        });

        let mut adv_builder = LegacyAdvertisementBuilder::new();
        adv_builder = adv_builder.flags(&[Flag::GeneralDiscovery, Flag::LE_Only]);
        
        if in_pairing_mode {
            adv_builder = adv_builder.services_16(
                ServiceList::Complete,
                &[nrf_softdevice::ble::advertisement_builder::ServiceUuid16::HUMAN_INTERFACE_DEVICE],
            );
        }
        
        adv_builder = adv_builder.services_128(
            ServiceList::Complete,
            &[[
                0x8a, 0x12, 0xd7, 0xba, 0x46, 0x77, 0x30, 0xad,
                0xe8, 0x46, 0x3e, 0x0b, 0x01, 0x00, 0x7a, 0x9e,
            ]],
        );

        if in_pairing_mode {
            adv_builder = adv_builder.raw(
                nrf_softdevice::ble::advertisement_builder::AdvertisementDataType::APPEARANCE,
                &[0xC1, 0x03], // Keyboard (0x03C1)
            );
        }

        let adv_payload = adv_builder.build();

        let config = peripheral::Config::default();
        crate::log_msg!("BLE Advertising started... (pairing_mode: {})", in_pairing_mode);

        let adv_fut = peripheral::advertise_pairable(
            sd,
            peripheral::ConnectableAdvertisement::ScannableUndirected {
                adv_data: &adv_payload,
                scan_data: &scan_payload,
            },
            &config,
            sec_handler_ref,
        );

        let conn = match select(adv_fut, BLE_COMMANDS.receive()).await {
            Either::First(Ok(c)) => c,
            Either::First(Err(e)) => {
                warn!("BLE advertising failed: {:?}", e);
                embassy_time::Timer::after(embassy_time::Duration::from_millis(500)).await;
                continue;
            }
            Either::Second(cmd) => {
                crate::log_msg!("BLE: Command received during advertising. Restarting advertising loop.");
                match cmd {
                    BleCommand::Disconnect | BleCommand::RestartAdvertising => {}
                    BleCommand::TypeString(_) => {
                        // Resend to channel or discard? Discard since no connection exists.
                    }
                }
                continue;
            }
        };

        crate::log_msg!("BLE Connection established from {:?}", conn.peer_address());

        // Check authorization
        let peer_addr = conn.peer_address();
        let is_authorized = KEYBOARD_STATE.lock(|state| {
            let s = state.borrow();
            let slot = s.active_slot;
            if let Some(ref bond) = s.bonds[slot] {
                bond.peer_id.is_match(peer_addr)
            } else {
                true
            }
        });

        if !is_authorized {
            crate::log_msg!("BLE: Unauthorized peer {:?}. Disconnecting.", peer_addr);
            let _ = conn.disconnect_with_reason(HciStatus::AUTHENTICATION_FAILURE);
            continue;
        }

        let server_ref = server;
        let conn_ref = &conn;

        let notify_task = async move {
            let mut val = 0u8;
            loop {
                embassy_time::Timer::after(embassy_time::Duration::from_secs(5)).await;
                val = val.wrapping_add(1);
                // Status notifications are a best-effort heartbeat on the
                // CustomService status characteristic. The HID host (macOS)
                // does not subscribe to this CCCD, so status_notify returns
                // Err(NotEnabled). That is expected and must NOT break this
                // task: breaking would complete the `select` that also drives
                // gatt_task/cmd_task, cancelling the GATT server and tearing
                // down the live BLE link (the flapping bug). Just skip and retry.
                if let Err(e) = server_ref.custom.status_notify(conn_ref, &val) {
                    trace!("BLE status notify skipped (not subscribed): {:?}", e);
                }
            }
        };

        let gatt_task = async {
            let e = gatt_server::run(conn_ref, server_ref, |event| match event {
                ServerEvent::Custom(e) => match e {
                    CustomServiceEvent::LedWrite(val) => {
                        crate::log_msg!("BLE LED command received: {}", val);
                        crate::LED_MODE.signal(val);
                    }
                    CustomServiceEvent::StatusCccdWrite { notifications } => {
                        crate::log_msg!("BLE status notification configuration changed: {}", notifications);
                    }
                },
                ServerEvent::Hid(e) => match e {
                    HidServiceEvent::InputReportCccdWrite { notifications } => {
                        crate::log_msg!("BLE input report notifications: {}", notifications);
                    }
                },
            })
            .await;
            let reason = conn_ref.disconnect_reason();
            crate::log_msg!("BLE connection closed; error: {:?}, reason: {:?}", e, reason);
        };

        let cmd_task = async {
            loop {
                let cmd = BLE_COMMANDS.receive().await;
                match cmd {
                    BleCommand::Disconnect | BleCommand::RestartAdvertising => {
                        crate::log_msg!("BLE: Disconnecting active connection");
                        let _ = conn_ref.disconnect();
                        break;
                    }
                    BleCommand::TypeString(s) => {
                        crate::log_msg!("BLE: Typing string: {}", s.as_str());
                        for c in s.chars() {
                            if let Some((mods, keycode)) = ascii_to_hid(c) {
                                let mut report = [0u8; 8];
                                report[0] = mods;
                                report[2] = keycode;
                                if let Err(e) = server_ref.hid.send_key(conn_ref, &report) {
                                    warn!("BLE key down notify error: {:?}", e);
                                    break;
                                }
                                embassy_time::Timer::after(embassy_time::Duration::from_millis(20)).await;

                                report[0] = 0;
                                report[2] = 0;
                                if let Err(e) = server_ref.hid.send_key(conn_ref, &report) {
                                    warn!("BLE key up notify error: {:?}", e);
                                    break;
                                }
                                embassy_time::Timer::after(embassy_time::Duration::from_millis(20)).await;
                            }
                        }
                    }
                }
            }
        };

        select(select(notify_task, gatt_task), cmd_task).await;
    }
}
