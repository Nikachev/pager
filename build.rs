use serde_json::Value;
use std::{env, fs, path::PathBuf};

fn number(layout: &Value, key: &str) -> u64 {
    layout[key]
        .as_u64()
        .unwrap_or_else(|| panic!("layout.json is missing numeric field {key}"))
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let layout: Value = serde_json::from_slice(&fs::read("layout.json").unwrap()).unwrap();
    let image_start = number(&layout, "firmware_image_start");
    let storage_start = number(&layout, "storage_start");
    let storage_size = number(&layout, "storage_size");
    let page_size = number(&layout, "flash_page_size");

    assert!(image_start < storage_start);
    assert_eq!(storage_start % page_size, 0);
    assert_eq!(storage_size % page_size, 0);

    fs::write(
        out.join("memory.x"),
        format!(
            "MEMORY {{\n  FLASH : ORIGIN = 0x{image_start:08X}, LENGTH = {flash_len}\n  STORAGE : ORIGIN = 0x{storage_start:08X}, LENGTH = {storage_size}\n  RAM : ORIGIN = 0x20000000, LENGTH = 256K\n}}\n__storage_start = ORIGIN(STORAGE);\n__storage_end = ORIGIN(STORAGE) + LENGTH(STORAGE);\n",
            flash_len = storage_start - image_start,
        ),
    )
    .unwrap();

    let protocol: Value = serde_json::from_slice(&fs::read("protocol.json").unwrap()).unwrap();
    let commands = &protocol["commands"];
    let errors = &protocol["errors"];
    let command = |name: &str| number(commands, name);
    let error = |name: &str| number(errors, name);
    fs::write(
        out.join("protocol_spec.rs"),
        format!(
            "pub const FRAME_VERSION: u8 = {frame_version};\npub const STATE_SCHEMA: u8 = {state_schema};\npub const MAX_PAYLOAD: usize = {max_payload};\npub const APPLICATION_VID: u16 = {vid};\npub const APPLICATION_PID: u16 = {pid};\npub const ERROR_BAD_REQUEST: u8 = {error_bad_request};\npub const ERROR_UNSUPPORTED_COMMAND: u8 = {error_unsupported};\npub const ERROR_BUSY: u8 = {error_busy};\npub const ERROR_HID_NOT_READY: u8 = {error_hid_not_ready};\npub const ERROR_UNSUPPORTED_CHARACTER: u8 = {error_unsupported_character};\npub const ERROR_CONNECTION_LOST: u8 = {error_connection_lost};\npub const ERROR_QUEUE_FULL: u8 = {error_queue_full};\npub const PING: u8 = {ping};\npub const GET_INFO: u8 = {get_info};\npub const GET_STATE: u8 = {get_state};\npub const ACTIVATE_SLOT: u8 = {activate};\npub const CANCEL_PAIRING: u8 = {cancel};\npub const SET_BLUETOOTH_ENABLED: u8 = {set_ble};\npub const CLEAR_SLOT: u8 = {clear};\npub const TYPE_TEXT: u8 = {type_text};\npub const REBOOT_TO_BOOTLOADER: u8 = {reboot};\npub const GET_LOGS: u8 = {logs};\npub const SET_DEVICE_NAME: u8 = {set_name};\npub const SET_SLOT_NAME: u8 = {set_slot_name};\npub const FACTORY_RESET: u8 = {factory_reset};\n",
            frame_version = number(&protocol, "frame_version"),
            state_schema = number(&protocol, "state_schema"),
            max_payload = number(&protocol, "max_payload"),
            vid = number(&protocol, "application_vid"),
            pid = number(&protocol, "application_pid"),
            error_bad_request = error("bad_request"),
            error_unsupported = error("unsupported_command"),
            error_busy = error("busy"),
            error_hid_not_ready = error("hid_not_ready"),
            error_unsupported_character = error("unsupported_character"),
            error_connection_lost = error("connection_lost"),
            error_queue_full = error("queue_full"),
            ping = command("ping"),
            get_info = command("get_info"),
            get_state = command("get_state"),
            activate = command("activate_slot"),
            cancel = command("cancel_pairing"),
            set_ble = command("set_bluetooth_enabled"),
            clear = command("clear_slot"),
            type_text = command("type_text"),
            reboot = command("reboot_to_bootloader"),
            logs = command("get_logs"),
            set_name = command("set_device_name"),
            set_slot_name = command("set_slot_name"),
            factory_reset = command("factory_reset"),
        ),
    )
    .unwrap();
    fs::write(
        out.join("layout.rs"),
        format!(
            "pub const FLASH_PAGE_SIZE: u32 = {page_size};\npub const FIRMWARE_START: u32 = {firmware_start};\npub const MANIFEST_SIZE: u32 = {manifest_size};\npub const FIRMWARE_IMAGE_START: u32 = {image_start};\npub const STORAGE_START: u32 = {storage_start};\npub const STORAGE_SIZE: u32 = {storage_size};\n",
            firmware_start = number(&layout, "firmware_start"),
            manifest_size = number(&layout, "manifest_size"),
        ),
    )
    .unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("arm") {
        println!("cargo:rustc-link-arg=-Tdefmt.x");
    }
    println!("cargo:rerun-if-changed=layout.json");
    println!("cargo:rerun-if-changed=protocol.json");
    println!("cargo:rerun-if-env-changed=PAGER_SKIP_WATCHDOG_FEED");
    println!(
        "cargo:rustc-env=PAGER_SKIP_WATCHDOG_FEED={}",
        env::var("PAGER_SKIP_WATCHDOG_FEED").unwrap_or_default()
    );
    println!("cargo:rerun-if-env-changed=PAGER_BUILD_VERSION");
    println!(
        "cargo:rustc-env=PAGER_BUILD_VERSION={}",
        env::var("PAGER_BUILD_VERSION").unwrap_or_else(|_| "dev-unknown".to_owned())
    );
}
