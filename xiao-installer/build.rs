use std::env;
use std::fs;
use std::path::PathBuf;

const FACTORY_APPLICATION_START: u32 = 0x0002_7000;
const FACTORY_BOOTLOADER_START: u32 = 0x000F_4000;

fn main() {
    // Host-only library tests do not embed or link a bootloader image.
    if env::var("TARGET").unwrap() != "thumbv7em-none-eabihf" {
        return;
    }
    let bootloader = PathBuf::from(
        env::var_os("PAGER_BOOTLOADER_BIN")
            .expect("PAGER_BOOTLOADER_BIN must point to the Pager bootloader binary"),
    );
    let bootloader = bootloader
        .canonicalize()
        .expect("canonicalize PAGER_BOOTLOADER_BIN");
    let bootloader_len = fs::metadata(&bootloader)
        .expect("read Pager bootloader metadata")
        .len();
    assert!(bootloader_len >= 8, "Pager bootloader binary is too short");
    assert!(
        bootloader_len <= 0xC000,
        "Pager bootloader does not fit its 48 KiB partition"
    );

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("memory.x"),
        format!(
            "MEMORY {{\n  FLASH : ORIGIN = 0x{FACTORY_APPLICATION_START:08X}, LENGTH = 0x{:X}\n  RAM : ORIGIN = 0x20000000, LENGTH = 256K\n}}\n",
            FACTORY_BOOTLOADER_START - FACTORY_APPLICATION_START
        ),
    )
    .expect("write installer memory.x");

    println!("cargo:rustc-link-search={}", out.display());
    println!(
        "cargo:rustc-env=PAGER_INSTALLER_BOOTLOADER_BIN={}",
        bootloader.display()
    );
    println!("cargo:rerun-if-env-changed=PAGER_BOOTLOADER_BIN");
    println!("cargo:rerun-if-changed={}", bootloader.display());
}
