use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use pager_bootloader_core::codec::{Manifest as ManifestHeader, Uf2Block, MANIFEST_MAGIC};
const XIAO_FACTORY_APPLICATION_START: u32 = 0x0002_7000;
const XIAO_FACTORY_BOOTLOADER_START: u32 = 0x000F_4000;

#[derive(Clone, Copy)]
struct Layout {
    bootloader_size: usize,
    firmware_start: u32,
    manifest_size: usize,
    storage_start: u32,
    storage_size: usize,
}

struct BuildVersion {
    display: String,
    numeric: u32,
}

mod execution;
use execution::*;
mod version;
use version::*;
mod package;
use package::*;
mod keys;
use keys::*;
mod layout;
use layout::*;
mod ui;
use ui::*;
mod checks;
use checks::*;
mod build;
use build::*;

fn main() {
    let args: Vec<String> = env::args().collect();
    let task = args.get(1).map(|s| s.as_str()).unwrap_or("build");

    let outcome = match task {
        "build" | "build-dev" => build_firmware(false),
        "build-release" => build_firmware(true),
        "bootloader-updater-dev" => build_updater(),
        "bootloader-dev" => build_bootloader(false),
        "bootloader-release" => build_bootloader(true),
        "xiao-installer-dev" => build_xiao_installer(false),
        "xiao-installer-release" => build_xiao_installer(true),
        "verify" => verify_uf2(),
        "verify-file" => match args.get(2) {
            Some(path) if std::path::Path::new(path).is_file() => {
                verify_uf2_path(&PathBuf::from(path))
            }
            _ => Err("verify-file requires an existing UF2 path".into()),
        },
        "size" => check_size_budgets(),
        "check-layout" => check_layout(),
        "check-protocol" => check_protocol(),
        "build-ui" => build_ui(),
        "check-ui" => check_ui(),
        "info" => print_memory_info(),
        _ => Err(format!("unknown xtask: {task}")),
    };
    if let Err(error) = outcome {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
