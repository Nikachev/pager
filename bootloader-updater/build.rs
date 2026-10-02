use std::{env, fs, path::PathBuf};
fn main() {
    if env::var("TARGET").unwrap() != "thumbv7em-none-eabihf" {
        return;
    }
    let layout: serde_json::Value =
        serde_json::from_slice(&fs::read("../layout.json").unwrap()).unwrap();
    let base = layout["firmware_image_start"].as_u64().unwrap();
    let end = layout["storage_start"].as_u64().unwrap();
    assert_eq!(layout["bootloader_size"].as_u64().unwrap(), 0xC000);
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("device.x"),
        "/* Interrupts are disabled in the updater. */\n",
    )
    .unwrap();
    fs::write(out.join("memory.x"), format!("MEMORY {{ FLASH : ORIGIN = {base}, LENGTH = {}\n RAM : ORIGIN = 0x20000000, LENGTH = 256K }}\n",end-base)).unwrap();
    let path =
        PathBuf::from(env::var_os("PAGER_BOOTLOADER_BIN").expect("bootloader binary required"))
            .canonicalize()
            .unwrap();
    let hash = env::var("PAGER_BOOTLOADER_HASH").expect("bootloader hash required");
    let serial = env::var("PAGER_USB_SERIAL").expect("target chip serial required");
    assert_eq!(serial.len(), 16);
    assert!(serial.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    let board = if env::var_os("CARGO_FEATURE_BOARD_NICE_NANO_V2").is_some() {
        1u32
    } else {
        2u32
    };
    let length = fs::metadata(&path).unwrap().len() as u32;
    let serial_value = u64::from_str_radix(&serial, 16).unwrap();
    let mut descriptor = b"PGRBLUP1".to_vec();
    descriptor.extend_from_slice(&serial_value.to_le_bytes());
    for i in 0..32 {
        descriptor.push(u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).unwrap());
    }
    descriptor.extend_from_slice(&board.to_le_bytes());
    descriptor.extend_from_slice(&length.to_le_bytes());
    fs::write(
        out.join("target.rs"),
        format!("const DESCRIPTOR: [u8;56] = {:?};\n", descriptor),
    )
    .unwrap();
    println!(
        "cargo:rustc-env=PAGER_UPDATER_BOOTLOADER={}",
        path.display()
    );
    println!("cargo:rustc-link-search={}", out.display());
    for name in [
        "PAGER_BOOTLOADER_BIN",
        "PAGER_BOOTLOADER_HASH",
        "PAGER_USB_SERIAL",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo:rerun-if-changed=../layout.json");
}
