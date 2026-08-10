use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const UF2_MAGIC_START0: u32 = 0x0A324655; // "UF2\n"
const UF2_MAGIC_START1: u32 = 0x9E5D5157;
const UF2_MAGIC_END: u32 = 0x0AB16F30;
const UF2_FLAG_FAMILY_ID_PRESENT: u32 = 0x00002000;
const NRF52840_FAMILY_ID: u32 = 0xADA52840;

const MANIFEST_MAGIC: [u8; 8] = *b"PGRFW002";

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

fn command_text(repo_root: &std::path::Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|_| panic!("run {program}"));
    assert!(output.status.success(), "{program} {:?} failed", args);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn build_version(repo_root: &std::path::Path) -> BuildVersion {
    let date = command_text(repo_root, "date", &["-u", "+%y %m %d%H%M%S"]);
    let mut parts = date.split_whitespace();
    let year = parts.next().unwrap();
    let month = parts.next().unwrap();
    let timestamp = parts.next().unwrap();
    let branch = command_text(repo_root, "git", &["rev-parse", "--abbrev-ref", "HEAD"]);
    let main_branch = "main";
    let clean = command_text(repo_root, "git", &["status", "--porcelain"]).is_empty();
    let base = if branch == main_branch {
        "HEAD".to_owned()
    } else {
        command_text(repo_root, "git", &["merge-base", "HEAD", main_branch])
    };
    let commit_dates = command_text(repo_root, "git", &["log", "--format=%cs", &base]);
    let prefix = format!("20{year}-{month}-");
    let ordinal = commit_dates
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .count() as u32;
    let numeric = year.parse::<u32>().unwrap() * 1_000_000
        + month.parse::<u32>().unwrap() * 10_000
        + ordinal.min(9_999);
    let release = branch == main_branch && clean;
    let base_display = format!("{year}.{month}.{ordinal}");
    BuildVersion {
        display: if release {
            base_display
        } else {
            format!("{base_display}-{timestamp}")
        },
        numeric,
    }
}

impl Layout {
    fn load(repo_root: &std::path::Path) -> Self {
        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(repo_root.join("layout.json")).expect("read layout.json"),
        )
        .expect("parse layout.json");
        let number = |key: &str| {
            value[key]
                .as_u64()
                .unwrap_or_else(|| panic!("layout.json is missing numeric field {key}"))
        };
        Self {
            bootloader_size: number("bootloader_size") as usize,
            firmware_start: number("firmware_start") as u32,
            manifest_size: number("manifest_size") as usize,
            storage_start: number("storage_start") as u32,
            storage_size: number("storage_size") as usize,
        }
    }

    fn application_partition_size(self) -> usize {
        self.storage_start as usize - self.firmware_start as usize
    }
}

#[repr(C, packed)]
struct ManifestHeader {
    magic: [u8; 8],
    version: u32,
    image_len: u32,
    digest: [u8; 32],
    signature: [u8; 64],
}

impl ManifestHeader {
    fn to_bytes(&self) -> [u8; 112] {
        let mut buf = [0u8; 112];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.image_len.to_le_bytes());
        buf[16..48].copy_from_slice(&self.digest);
        buf[48..112].copy_from_slice(&self.signature);
        buf
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let task = args.get(1).map(|s| s.as_str()).unwrap_or("build");

    match task {
        "build" | "build-dev" => build_firmware(false),
        "build-release" => build_firmware(true),
        "bootloader-dev" => build_bootloader(false),
        "bootloader-release" => build_bootloader(true),
        "verify" => verify_uf2(),
        "size" => check_size_budgets(),
        "check-layout" => check_layout(),
        "check-protocol" => check_protocol(),
        "build-ui" => build_ui(),
        "check-ui" => check_ui(),
        "info" => print_memory_info(),
        _ => {
            eprintln!("Unknown xtask: {task}");
            std::process::exit(1);
        }
    }
}

fn print_memory_info() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let signed_bin_path = repo_root.join("dist/pager-signed.bin");
    let layout = Layout::load(&repo_root);

    println!("==================================================");
    println!("     Pager Memory Partitioning & Utilization     ");
    println!("==================================================");

    let app_limit = layout.application_partition_size();
    let boot_limit = layout.bootloader_size;

    if signed_bin_path.exists() {
        let size = fs::metadata(&signed_bin_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let pct = (size as f64 / app_limit as f64) * 100.0;
        let bar_filled = ((pct / 5.0) as usize).min(20);
        let bar = "█".repeat(bar_filled) + &"░".repeat(20 - bar_filled);
        println!("📦 Application Partition (0x0C000..0xFE000):");
        println!("   Size: {} / {} bytes ({:.1}%)", size, app_limit, pct);
        println!("   [{}]", bar);
    } else {
        println!(
            "📦 Application Partition: dist/pager-signed.bin not built yet (run 'make build')"
        );
    }

    let raw_boot_path = repo_root.join("dist/pager-bootloader.bin");
    let target_boot_path =
        repo_root.join("bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader");
    let boot_path = if raw_boot_path.exists() {
        raw_boot_path
    } else {
        target_boot_path
    };

    if boot_path.exists() {
        let raw_size = fs::read(&boot_path)
            .map(|b| {
                if b.starts_with(b"\x7fELF") {
                    elf_binary_size(&b).unwrap_or(b.len())
                } else {
                    b.len()
                }
            })
            .unwrap_or(0);
        let pct = (raw_size as f64 / boot_limit as f64) * 100.0;
        let bar_filled = ((pct / 5.0) as usize).min(20);
        let bar = "█".repeat(bar_filled) + &"░".repeat(20 - bar_filled);
        println!("\n⚡ Bootloader Partition (0x00000..0x0C000):");
        println!("   Size: {} / {} bytes ({:.1}%)", raw_size, boot_limit, pct);
        println!("   [{}]", bar);
    } else {
        println!("\n⚡ Bootloader Partition: bootloader release binary not built yet (run 'make bootloader')");
    }

    println!(
        "\n💾 Storage Partition: {} bytes (Persistent BLE Bonds & Settings)",
        layout.storage_size
    );
    println!("==================================================");
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn check_layout() {
    let layout = Layout::load(&repo_root());
    assert_eq!(layout.firmware_start as usize, layout.bootloader_size);
    assert_eq!(layout.manifest_size, 256);
    assert_eq!(layout.storage_start as usize % 4096, 0);
    assert_eq!(layout.storage_size % 4096, 0);
    assert!(layout.storage_start as usize + layout.storage_size <= 1024 * 1024);
    assert!(layout.application_partition_size() > layout.manifest_size);
    println!("layout.json: OK");
}

fn check_protocol() {
    let root = repo_root();
    let protocol: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("protocol.json")).expect("read protocol.json"))
            .expect("parse protocol.json");
    let frame = protocol["frame_version"].as_u64().unwrap();
    let schema = protocol["state_schema"].as_u64().unwrap();
    let vid = protocol["application_vid"].as_u64().unwrap();
    let pid = protocol["application_pid"].as_u64().unwrap();
    let python = fs::read_to_string(root.join("pager_tools/protocol.py")).unwrap();
    let usb_python = fs::read_to_string(root.join("pager_tools/usb.py")).unwrap();
    let html = fs::read_to_string(root.join("webusb_client.html")).unwrap();
    let ble_html = fs::read_to_string(root.join("ble_client.html")).unwrap();
    assert!(python.contains(&format!("VERSION = {frame}")));
    assert!(html.contains(&format!("VERSION={frame}")));
    assert!(html.contains(&format!("bytes[0]!=={schema}")));
    assert!(html.contains(&format!("VID=0x{vid:04x},PID=0x{pid:04x}")));
    assert!(usb_python.contains(&format!("APP_VID = 0x{vid:04X}")));
    assert!(usb_python.contains(&format!("APP_PID = 0x{pid:04X}")));
    for (name, page) in [
        ("webusb_client.html", &html),
        ("ble_client.html", &ble_html),
    ] {
        assert!(
            page.contains("<!doctype html>"),
            "{name} is not standalone HTML"
        );
        assert!(
            !page.contains("http://") && !page.contains("https://"),
            "{name} contains an external resource"
        );
    }
    assert!(html.contains("id=\"disconnect\""));
    assert!(html.contains("releaseInterface"));
    for (name, js_name) in [
        ("get_state", "GET_STATE"),
        ("activate_slot", "ACTIVATE_SLOT"),
        ("cancel_pairing", "CANCEL_PAIRING"),
        ("set_bluetooth_enabled", "SET_BLUETOOTH"),
        ("clear_slot", "CLEAR_SLOT"),
        ("type_text", "TYPE_TEXT"),
        ("reboot_to_bootloader", "REBOOT"),
        ("get_logs", "GET_LOGS"),
        ("set_device_name", "SET_NAME"),
        ("set_slot_name", "SET_SLOT_NAME"),
        ("factory_reset", "FACTORY_RESET"),
    ] {
        let value = protocol["commands"][name].as_u64().unwrap();
        assert!(html.contains(&format!("{js_name}={value}")));
    }
    println!("protocol.json: Rust/Python/standalone JS constants agree");
}

fn check_ui() {
    let root = repo_root();
    let webusb =
        fs::read_to_string(root.join("webusb_client.html")).expect("read webusb_client.html");
    let ble = fs::read_to_string(root.join("ble_client.html")).expect("read ble_client.html");

    for (name, page) in [("webusb_client.html", &webusb), ("ble_client.html", &ble)] {
        assert!(
            page.contains("<!doctype html>"),
            "{name} must be standalone HTML"
        );
        assert!(page.contains("<style>"), "{name} must embed its CSS");
        assert!(
            !page.contains("http://") && !page.contains("https://"),
            "{name} must not depend on network resources"
        );
        assert!(
            page.contains("--bg:#f6f7f9"),
            "{name} must use the shared design tokens"
        );
    }

    for contract in [
        "aria-live=\"polite\"",
        "<dialog id=\"confirm_dialog\"",
        "releaseInterface",
        "Device response timeout",
        "pendingCalls=new Map()",
        "lastRevision",
        "overflow||gap",
        "encoded.length>24",
        "encoded.length>32",
        "This control page is incompatible",
        "disabled=!state.hidReady",
    ] {
        assert!(
            webusb.contains(contract),
            "WebUSB UI contract is missing {contract:?}"
        );
    }
    for contract in ["macOS", "Android", "HID Keyboard", "13%", "Pager 1"] {
        assert!(
            ble.contains(contract),
            "BLE UI contract is missing {contract:?}"
        );
    }
    for (name, source) in [
        ("webusb_client.html", webusb.as_bytes()),
        ("ble_client.html", ble.as_bytes()),
    ] {
        let artifact = fs::read(root.join("dist/ui").join(name))
            .unwrap_or_else(|_| panic!("missing dist/ui/{name}; run xtask build-ui"));
        assert_eq!(artifact, source, "dist/ui/{name} is stale");
    }
    println!("standalone UI artifacts and interaction contracts: OK");
}

fn build_ui() {
    let root = repo_root();
    let output = root.join("dist/ui");
    fs::create_dir_all(&output).expect("create dist/ui");
    for name in ["webusb_client.html", "ble_client.html"] {
        let source = fs::read(root.join(name)).unwrap_or_else(|_| panic!("read {name}"));
        fs::write(output.join(name), source).unwrap_or_else(|_| panic!("write dist/ui/{name}"));
    }
    println!("standalone UI artifacts written to dist/ui");
}

fn binary_size(path: &std::path::Path) -> usize {
    let bytes = fs::read(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    if bytes.starts_with(b"\x7fELF") {
        elf_binary_size(&bytes).unwrap_or(bytes.len())
    } else {
        bytes.len()
    }
}

fn check_size_budgets() {
    let root = repo_root();
    let layout = Layout::load(&root);
    let app = root.join("dist/pager-signed.bin");
    let boot = root.join("bootloader/target/thumbv7em-none-eabihf/release/pager-bootloader");
    let app_size = binary_size(&app);
    let boot_size = binary_size(&boot);
    let app_budget = layout.application_partition_size() - 16 * 1024;
    let boot_budget = layout.bootloader_size - 1024;
    assert!(
        app_size <= app_budget,
        "application size {app_size} exceeds budget {app_budget}"
    );
    assert!(
        boot_size <= boot_budget,
        "bootloader size {boot_size} exceeds budget {boot_budget}"
    );
    println!(
        "size budgets: application {app_size}/{app_budget}, bootloader {boot_size}/{boot_budget}"
    );
}

fn decode_hex_32(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn verify_uf2() {
    let root = repo_root();
    let layout = Layout::load(&root);
    let uf2 = fs::read(root.join("dist/pager.uf2")).expect("read dist/pager.uf2");
    assert!(
        !uf2.is_empty() && uf2.len() % 512 == 0,
        "invalid UF2 length"
    );
    let count = uf2.len() / 512;
    let mut image = vec![0u8; count * 256];
    let mut seen = vec![false; count];
    for block in uf2.chunks_exact(512) {
        assert_eq!(
            u32::from_le_bytes(block[0..4].try_into().unwrap()),
            UF2_MAGIC_START0
        );
        assert_eq!(
            u32::from_le_bytes(block[4..8].try_into().unwrap()),
            UF2_MAGIC_START1
        );
        assert_eq!(
            u32::from_le_bytes(block[508..512].try_into().unwrap()),
            UF2_MAGIC_END
        );
        assert_eq!(
            u32::from_le_bytes(block[28..32].try_into().unwrap()),
            NRF52840_FAMILY_ID
        );
        let payload = u32::from_le_bytes(block[16..20].try_into().unwrap()) as usize;
        let number = u32::from_le_bytes(block[20..24].try_into().unwrap()) as usize;
        let total = u32::from_le_bytes(block[24..28].try_into().unwrap()) as usize;
        let address = u32::from_le_bytes(block[12..16].try_into().unwrap());
        assert_eq!(total, count);
        assert!(number < count && !seen[number]);
        assert!(payload <= 256);
        assert_eq!(address, layout.firmware_start + (number * 256) as u32);
        image[number * 256..number * 256 + payload].copy_from_slice(&block[32..32 + payload]);
        seen[number] = true;
    }
    assert!(seen.into_iter().all(|value| value));
    assert_eq!(&image[0..8], &MANIFEST_MAGIC);
    let image_len = u32::from_le_bytes(image[12..16].try_into().unwrap()) as usize;
    assert!(layout.manifest_size + image_len <= image.len());
    let digest: [u8; 32] =
        Sha256::digest(&image[layout.manifest_size..layout.manifest_size + image_len]).into();
    assert_eq!(&image[16..48], &digest);
    let signature = Signature::from_bytes(image[48..112].try_into().unwrap());
    let mut trusted = Vec::new();
    for path in [
        root.join("bootloader/firmware_signing_public.hex"),
        root.join("keys/dev_signing_public.hex"),
    ] {
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(bytes) = decode_hex_32(&text) {
                trusted.push(VerifyingKey::from_bytes(&bytes).expect("valid public key"));
            }
        }
    }
    assert!(
        trusted
            .iter()
            .any(|key| key.verify(&image[..48], &signature).is_ok()),
        "UF2 signature is not trusted by the local dev/release key set"
    );
    println!(
        "dist/pager.uf2: signature, digest, layout and {} blocks OK",
        count
    );
}

fn elf_binary_size(elf: &[u8]) -> Option<usize> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" {
        return None;
    }
    let phoff = u32::from_le_bytes(elf[0x1C..0x20].try_into().ok()?) as usize;
    let phentsize = u16::from_le_bytes(elf[0x2A..0x2C].try_into().ok()?) as usize;
    let phnum = u16::from_le_bytes(elf[0x2C..0x2E].try_into().ok()?) as usize;

    let mut total_size = 0;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 32 > elf.len() {
            break;
        }
        let p_type = u32::from_le_bytes(elf[off..off + 4].try_into().ok()?);
        if p_type == 1 {
            let p_filesz = u32::from_le_bytes(elf[off + 16..off + 20].try_into().ok()?) as usize;
            total_size += p_filesz;
        }
    }
    if total_size > 0 {
        Some(total_size)
    } else {
        None
    }
}

fn build_firmware(release: bool) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let dist_dir = repo_root.join("dist");
    let layout = Layout::load(&repo_root);
    let version = build_version(&repo_root);
    fs::create_dir_all(&dist_dir).expect("Failed to create dist dir");

    println!("==================================================");
    println!("     Building Pager Single-Slot Firmware & UF2    ");
    println!("==================================================");

    let board = env::var("PAGER_BOARD").unwrap_or_else(|_| "nice-nano-v2".to_owned());
    let board_feature = match board.as_str() {
        "nice-nano-v2" => "board-nice-nano-v2",
        "xiao-nrf52840" => "board-xiao-nrf52840",
        _ => panic!("PAGER_BOARD must be nice-nano-v2 or xiao-nrf52840"),
    };
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--no-default-features",
            "--features",
            board_feature,
        ])
        .env("PAGER_BUILD_VERSION", &version.display)
        .current_dir(&repo_root)
        .status()
        .expect("Failed to run cargo build");

    if !status.success() {
        eprintln!("Cargo build failed");
        std::process::exit(1);
    }

    let elf_path = repo_root.join("target/thumbv7em-none-eabihf/release/pager");
    let raw_bin_path = dist_dir.join("pager-raw.bin");
    let uf2_path = dist_dir.join("pager.uf2");

    let objcopy_res = Command::new("rust-objcopy")
        .args([
            "-O",
            "binary",
            elf_path.to_str().unwrap(),
            raw_bin_path.to_str().unwrap(),
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
                raw_bin_path.to_str().unwrap(),
            ])
            .current_dir(&repo_root)
            .status();
        if fallback.is_err() || !fallback.unwrap().success() {
            eprintln!("Failed to extract binary with rust-objcopy/cargo-objcopy");
            std::process::exit(1);
        }
    }

    let raw_bin = fs::read(&raw_bin_path).expect("Failed to read raw .bin");
    let image_len = raw_bin.len() as u32;

    // 1. Calculate SHA-256 Digest of raw binary payload
    let mut hasher = Sha256::new();
    hasher.update(&raw_bin);
    let digest_bytes: [u8; 32] = hasher.finalize().into();

    // 2. Load signing key from PEM file or fallback to Dev Key
    let signing_key = load_signing_key(&repo_root, release);
    let verifying_key = signing_key.verifying_key();
    println!(
        "🔑 Signing Firmware Public Key: {:?}",
        verifying_key.to_bytes()
    );

    // Construct signed message: Magic + Version + image_len + digest
    let mut signed_msg = Vec::new();
    signed_msg.extend_from_slice(&MANIFEST_MAGIC);
    signed_msg.extend_from_slice(&version.numeric.to_le_bytes());
    signed_msg.extend_from_slice(&image_len.to_le_bytes());
    signed_msg.extend_from_slice(&digest_bytes);

    let signature = signing_key.sign(&signed_msg);

    let manifest = ManifestHeader {
        magic: MANIFEST_MAGIC,
        version: version.numeric,
        image_len,
        digest: digest_bytes,
        signature: signature.to_bytes(),
    };

    let manifest_bytes = manifest.to_bytes();

    let mut full_image = Vec::new();
    full_image.extend_from_slice(&manifest_bytes);
    full_image.resize(layout.manifest_size, 0xFF);
    full_image.extend_from_slice(&raw_bin);

    // Generate UF2 blocks (512 bytes per block, 256 bytes payload)
    let payload_size = 256;
    let num_blocks = (full_image.len() + payload_size - 1) / payload_size;
    let mut uf2_data = Vec::new();

    for (block_no, chunk) in full_image.chunks(payload_size).enumerate() {
        let target_addr = layout.firmware_start + (block_no * payload_size) as u32;

        let mut block = [0u8; 512];
        block[0..4].copy_from_slice(&UF2_MAGIC_START0.to_le_bytes());
        block[4..8].copy_from_slice(&UF2_MAGIC_START1.to_le_bytes());
        block[8..12].copy_from_slice(&UF2_FLAG_FAMILY_ID_PRESENT.to_le_bytes());
        block[12..16].copy_from_slice(&target_addr.to_le_bytes());
        block[16..20].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        block[20..24].copy_from_slice(&(block_no as u32).to_le_bytes());
        block[24..28].copy_from_slice(&(num_blocks as u32).to_le_bytes());
        block[28..32].copy_from_slice(&NRF52840_FAMILY_ID.to_le_bytes());

        block[32..32 + chunk.len()].copy_from_slice(chunk);
        block[508..512].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());

        uf2_data.extend_from_slice(&block);
    }

    let signed_bin_path = dist_dir.join("pager-signed.bin");
    fs::write(&signed_bin_path, &full_image).expect("Failed to write pager-signed.bin");
    fs::write(&uf2_path, &uf2_data).expect("Failed to write .uf2 file");
    fs::write(
        dist_dir.join("version.txt"),
        format!("{}\n", version.display),
    )
    .expect("write build version");

    let max_firmware_size = layout.application_partition_size();
    if full_image.len() > max_firmware_size {
        eprintln!(
            "❌ ERROR: Signed firmware size ({} bytes) exceeds maximum partition slot size ({} bytes)!",
            full_image.len(), max_firmware_size
        );
        std::process::exit(1);
    }

    println!("==================================================");
    println!("🎉 UF2 Firmware Build Success!");
    println!("🏷️  Version: {}", version.display);
    println!(
        "📦 Signed Payload Size: {} bytes (limit: {} bytes)",
        full_image.len(),
        max_firmware_size
    );
    println!("==================================================");
}

fn load_signing_key(repo_root: &std::path::Path, release: bool) -> SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;

    if !release {
        return ensure_dev_signing_key(repo_root);
    }

    let env_key_path = env::var("PAGER_SIGNING_KEY").ok().map(PathBuf::from);
    let default_pem_path = repo_root.join("keys/firmware_signing_private.pem");

    let key_path = env_key_path.or_else(|| {
        if default_pem_path.exists() {
            Some(default_pem_path)
        } else {
            None
        }
    });

    if let Some(path) = key_path {
        if let Ok(pem_content) = fs::read_to_string(&path) {
            if let Ok(key) = SigningKey::from_pkcs8_pem(&pem_content) {
                println!(
                    "🔑 Signed firmware using PEM Private Key from: {}",
                    path.display()
                );
                assert_release_signing_key(repo_root, &key);
                return key;
            } else if let Ok(key_bytes) = fs::read(&path) {
                if key_bytes.len() == 32 {
                    let key = SigningKey::from_bytes(&key_bytes.try_into().unwrap());
                    println!(
                        "🔑 Signed firmware using raw 32-byte Private Key from: {}",
                        path.display()
                    );
                    assert_release_signing_key(repo_root, &key);
                    return key;
                }
            }
            panic!("failed to parse release signing key at {}", path.display());
        }
    }
    panic!("release signing requires PAGER_SIGNING_KEY or keys/firmware_signing_private.pem")
}

fn assert_release_signing_key(repo_root: &std::path::Path, key: &SigningKey) {
    let expected = fs::read_to_string(repo_root.join("bootloader/firmware_signing_public.hex"))
        .expect("read release public key");
    let expected = decode_hex_32(&expected).expect("release public key must be 32-byte hex");
    assert_eq!(
        key.verifying_key().to_bytes(),
        expected,
        "release private key does not match bootloader/firmware_signing_public.hex"
    );
}

fn ensure_dev_signing_key(repo_root: &std::path::Path) -> SigningKey {
    use std::io::Read;

    let key_dir = repo_root.join("keys");
    let private_path = key_dir.join("dev_signing_private.bin");
    let public_path = key_dir.join("dev_signing_public.hex");
    fs::create_dir_all(&key_dir).expect("create keys directory");

    let private = if private_path.exists() {
        let bytes = fs::read(&private_path).expect("read dev private key");
        assert_eq!(bytes.len(), 32, "dev private key must be 32 bytes");
        bytes.try_into().unwrap()
    } else {
        let mut bytes = [0u8; 32];
        fs::File::open("/dev/urandom")
            .expect("open OS random source")
            .read_exact(&mut bytes)
            .expect("read OS random source");
        fs::write(&private_path, bytes).expect("write dev private key");
        println!(
            "Created local development signing key at {}",
            private_path.display()
        );
        bytes
    };
    let key = SigningKey::from_bytes(&private);
    let public_hex: String = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fs::write(&public_path, format!("{public_hex}\n")).expect("write dev public key");
    key
}

fn build_bootloader(release: bool) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    if !release {
        let _ = ensure_dev_signing_key(&repo_root);
    }
    let mut command = Command::new("cargo");
    let board = env::var("PAGER_BOARD").unwrap_or_else(|_| "nice-nano-v2".to_owned());
    let board_feature = match board.as_str() {
        "nice-nano-v2" => "board-nice-nano-v2",
        "xiao-nrf52840" => "board-xiao-nrf52840",
        _ => panic!("PAGER_BOARD must be nice-nano-v2 or xiao-nrf52840"),
    };
    let kind = if release {
        "release-bootloader"
    } else {
        "dev-bootloader"
    };
    let features = format!("{kind},{board_feature}");
    command
        .args([
            "build",
            "--manifest-path",
            "bootloader/Cargo.toml",
            "--release",
            "--no-default-features",
            "--features",
            &features,
        ])
        .current_dir(&repo_root);
    assert!(command.status().expect("run bootloader build").success());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signature;

    #[test]
    fn test_manifest_header_packing_and_signature() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let verifying_key = signing_key.verifying_key();

        let raw_bin = b"Hello Pager Firmware Payload";
        let mut hasher = Sha256::new();
        hasher.update(raw_bin);
        let digest_bytes: [u8; 32] = hasher.finalize().into();

        let mut signed_msg = Vec::new();
        signed_msg.extend_from_slice(&MANIFEST_MAGIC);
        signed_msg.extend_from_slice(&1u32.to_le_bytes());
        signed_msg.extend_from_slice(&(raw_bin.len() as u32).to_le_bytes());
        signed_msg.extend_from_slice(&digest_bytes);

        let signature = signing_key.sign(&signed_msg);

        let manifest = ManifestHeader {
            magic: MANIFEST_MAGIC,
            version: 1,
            image_len: raw_bin.len() as u32,
            digest: digest_bytes,
            signature: signature.to_bytes(),
        };

        assert_eq!(core::mem::size_of::<ManifestHeader>(), 112);
        assert!(verifying_key
            .verify_strict(&signed_msg, &Signature::from_bytes(&manifest.signature))
            .is_ok());
        let bootloader_key = ed25519_compact::PublicKey::new(verifying_key.to_bytes());
        let bootloader_signature = ed25519_compact::Signature::new(manifest.signature);
        assert!(bootloader_key
            .verify(&signed_msg, &bootloader_signature)
            .is_ok());
    }
}
