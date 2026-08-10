use serde_json::Value;
use std::{env, fs, path::PathBuf, process::Command};

fn number(layout: &Value, key: &str) -> u64 {
    layout[key]
        .as_u64()
        .unwrap_or_else(|| panic!("layout.json is missing numeric field {key}"))
}

fn decode_public_key(path: &str) -> [u8; 32] {
    let text = fs::read_to_string(path).unwrap_or_else(|_| panic!("missing public key: {path}"));
    let text = text.trim();
    assert_eq!(text.len(), 64, "public key must contain 64 hex digits");
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .expect("invalid public key hex");
    }
    key
}

fn rust_key(key: [u8; 32]) -> String {
    let bytes = key.map(|byte| format!("0x{byte:02x}")).join(",");
    format!("[{bytes}]")
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let layout_path = PathBuf::from("../layout.json");
    let layout: Value = serde_json::from_slice(&fs::read(&layout_path).unwrap()).unwrap();
    let bootloader_size = number(&layout, "bootloader_size");
    let firmware_start = number(&layout, "firmware_start");
    let storage_start = number(&layout, "storage_start");
    assert_eq!(bootloader_size, firmware_start);

    fs::write(
        out.join("memory.x"),
        format!(
            "MEMORY {{\n  FLASH : ORIGIN = 0x00000000, LENGTH = {bootloader_size}\n  RAM : ORIGIN = 0x20000000, LENGTH = 256K\n}}\n"
        ),
    )
    .unwrap();
    fs::write(
        out.join("layout.rs"),
        format!(
            "pub const FIRMWARE_START: u32 = {firmware_start};\npub const FIRMWARE_END: u32 = {storage_start};\npub const PAGE_SIZE: u32 = {page_size};\npub const MANIFEST_SIZE: u32 = {manifest_size};\n",
            page_size = number(&layout, "flash_page_size"),
            manifest_size = number(&layout, "manifest_size"),
        ),
    )
    .unwrap();

    let dev = env::var_os("CARGO_FEATURE_DEV_BOOTLOADER").is_some();
    let release = env::var_os("CARGO_FEATURE_RELEASE_BOOTLOADER").is_some();
    assert_ne!(dev, release, "select exactly one bootloader kind");
    let release_key = decode_public_key("firmware_signing_public.hex");
    let mut keys = rust_key(release_key);
    if dev {
        keys.push(',');
        keys.push_str(&rust_key(decode_public_key("../keys/dev_signing_public.hex")));
    }
    fs::write(
        out.join("signing_keys.rs"),
        format!(
            "pub const BOOTLOADER_KIND: &str = \"{}\";\npub const FIRMWARE_SIGNING_PUBLIC_KEYS: &[[u8; 32]] = &[{keys}];\n",
            if dev { "dev" } else { "release" }
        ),
    )
    .unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed={}", layout_path.display());
    println!("cargo:rerun-if-changed=firmware_signing_public.hex");
    if dev {
        println!("cargo:rerun-if-changed=../keys/dev_signing_public.hex");
    }
    println!("cargo:rerun-if-env-changed=PAGER_POWER_CUT_TEST");
    println!("cargo:rustc-check-cfg=cfg(power_cut_test)");
    if env::var("PAGER_POWER_CUT_TEST").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=power_cut_test");
    }
    let build_date = Command::new("date")
        .args(["-u", "+%Y-%m-%d %H:%M:%S UTC"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=BUILD_DATE={build_date}");
}
