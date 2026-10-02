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
    let mut generated = String::new();
    for (key, ty, maximum) in [
        ("frame_version", "u8", 255),
        ("state_schema", "u8", 255),
        ("max_payload", "usize", 65535),
        ("header_size", "usize", 65535),
        ("application_vid", "u16", 65535),
        ("application_pid", "u16", 65535),
    ] {
        let value = number(&protocol, key);
        assert!(value > 0 && value <= maximum, "invalid protocol {key}");
        generated.push_str(&format!(
            "pub const {}: {ty} = {value};\n",
            key.to_uppercase()
        ));
    }
    let magic = protocol["frame_magic"].as_str().expect("frame magic");
    assert!(magic.is_ascii() && magic.len() == 4);
    assert_eq!(number(&protocol, "header_size"), 16);
    generated.push_str(&format!("pub const FRAME_MAGIC: [u8; 4] = *b{magic:?};\n"));
    for (section, prefix) in [
        ("commands", ""),
        ("errors", "ERROR_"),
        ("frame_kinds", "KIND_"),
    ] {
        let mut seen = std::collections::BTreeSet::new();
        for (name, value) in protocol[section].as_object().expect("protocol table") {
            let value = value.as_u64().expect("numeric protocol value");
            assert!(
                value > 0 && value <= 255 && seen.insert(value),
                "invalid/duplicate {section}"
            );
            generated.push_str(&format!(
                "pub const {prefix}{}: u8 = {value};\n",
                name.to_uppercase()
            ));
        }
    }
    for (name, value) in protocol["limits"].as_object().expect("limits") {
        let value = value.as_u64().expect("numeric limit");
        assert!(value > 0 && value <= number(&protocol, "max_payload"));
        generated.push_str(&format!(
            "pub const LIMIT_{}: usize = {value};\n",
            name.to_uppercase()
        ));
    }
    assert_eq!(number(&protocol["limits"], "slots"), 3);
    fs::write(out.join("protocol_spec.rs"), generated).unwrap();
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
    println!("cargo:rerun-if-env-changed=PAGER_FAULT_INJECTION");
    println!("cargo:rerun-if-env-changed=PAGER_USB_TRACE");
    if env::var("PAGER_USB_TRACE").as_deref() == Ok("1") {
        println!("cargo:rustc-env=PAGER_USB_TRACE=1");
    }
    if env::var("PAGER_FAULT_INJECTION").as_deref() == Ok("1") {
        println!("cargo:rustc-env=PAGER_FAULT_INJECTION=1");
    }
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
