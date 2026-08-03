use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    let task = args.get(1).map(|s| s.as_str()).unwrap_or("build");

    match task {
        "build" => build_firmware(),
        _ => {
            eprintln!("Unknown xtask: {task}");
            std::process::exit(1);
        }
    }
}

fn bin_to_ihex(bin_data: &[u8], base_address: u32) -> String {
    let mut ihex = String::new();
    let mut current_ext_addr: u16 = 0xFFFF;

    for (chunk_index, chunk) in bin_data.chunks(16).enumerate() {
        let addr = base_address + (chunk_index * 16) as u32;
        let ext_addr = (addr >> 16) as u16;
        let offset = (addr & 0xFFFF) as u16;

        if ext_addr != current_ext_addr {
            current_ext_addr = ext_addr;
            let high = (ext_addr >> 8) as u8;
            let low = (ext_addr & 0xFF) as u8;
            let len = 2u8;
            let rec_type = 4u8;
            let sum = len as u32 + rec_type as u32 + high as u32 + low as u32;
            let checksum = ((256 - (sum % 256)) % 256) as u8;
            ihex.push_str(&format!(
                ":02000004{:02X}{:02X}{:02X}\n",
                high, low, checksum
            ));
        }

        let len = chunk.len() as u8;
        let offset_high = (offset >> 8) as u8;
        let offset_low = (offset & 0xFF) as u8;
        let rec_type = 0u8;

        let mut sum = len as u32 + offset_high as u32 + offset_low as u32 + rec_type as u32;
        let mut data_hex = String::new();
        for &b in chunk {
            sum += b as u32;
            data_hex.push_str(&format!("{:02X}", b));
        }
        let checksum = ((256 - (sum % 256)) % 256) as u8;
        ihex.push_str(&format!(
            ":{:02X}{:04X}00{}{:02X}\n",
            len, offset, data_hex, checksum
        ));
    }

    ihex.push_str(":00000001FF\n");
    ihex
}

fn build_firmware() {
    let slot = env::var("PAGER_SLOT").unwrap_or_else(|_| "A".into());
    if slot != "A" && slot != "B" {
        eprintln!("PAGER_SLOT must be A or B");
        std::process::exit(2);
    }

    let base_address: u32 = if slot == "A" {
        0x0000_9000
    } else {
        0x0008_3000
    };

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let dist_dir = repo_root.join("dist");
    fs::create_dir_all(&dist_dir).expect("Failed to create dist dir");

    println!("==================================================");
    println!("     Building Pager Firmware via xtask (Slot {slot})");
    println!("==================================================");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .env("PAGER_SLOT", &slot)
        .current_dir(&repo_root)
        .status()
        .expect("Failed to run cargo build");

    if !status.success() {
        eprintln!("Cargo build failed");
        std::process::exit(1);
    }

    let elf_path = repo_root.join("target/thumbv7em-none-eabihf/release/pager");
    let bin_path = dist_dir.join(format!("pager-{slot}.bin"));
    let hex_path = dist_dir.join(format!("pager-{slot}.hex"));

    let objcopy_res = Command::new("rust-objcopy")
        .args([
            "-O",
            "binary",
            elf_path.to_str().unwrap(),
            bin_path.to_str().unwrap(),
        ])
        .status();

    if objcopy_res.is_err() || !objcopy_res.unwrap().success() {
        let fallback = Command::new("cargo")
            .args([
                "objcopy",
                "--release",
                "--bin",
                "pager",
                "--target",
                "thumbv7em-none-eabihf",
                "--",
                "-O",
                "binary",
                bin_path.to_str().unwrap(),
            ])
            .current_dir(&repo_root)
            .status();
        if fallback.is_err() || !fallback.unwrap().success() {
            eprintln!("Failed to extract binary with rust-objcopy/cargo-objcopy");
            std::process::exit(1);
        }
    }

    let bin_bytes = fs::read(&bin_path).expect("Failed to read generated .bin");
    let bin_size = bin_bytes.len();
    let max_size = 484 * 1024;
    if bin_size > max_size {
        eprintln!("Error: binary is {bin_size} bytes, capacity is {max_size} bytes");
        std::process::exit(1);
    }

    // Pure Rust Intel HEX generation
    let hex_content = bin_to_ihex(&bin_bytes, base_address);
    fs::write(&hex_path, hex_content).expect("Failed to write .hex file");

    let mut hasher = Sha256::new();
    hasher.update(&bin_bytes);
    let hash = format!("{:x}", hasher.finalize());

    println!("==================================================");
    println!("🎉 xtask build complete! Output files in dist/:");
    println!(
        "📂 {} ({} bytes, SHA-256: {})",
        bin_path.display(),
        bin_size,
        hash
    );
    println!("📂 {}", hex_path.display());
    println!("==================================================");
}
