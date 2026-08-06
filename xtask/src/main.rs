use ed25519_dalek::{Signer, SigningKey};
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

const FIRMWARE_START: u32 = 0x0000_C000;
const MANIFEST_MAGIC: [u8; 8] = *b"PGRFW001";

#[repr(C, packed)]
struct ManifestHeader {
    state: u32,
    magic: [u8; 8],
    version: u32,
    image_len: u32,
    target_slot: u32,
    digest: [u8; 32],
    signature: [u8; 64],
}

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

fn build_firmware() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let dist_dir = repo_root.join("dist");
    fs::create_dir_all(&dist_dir).expect("Failed to create dist dir");

    println!("==================================================");
    println!("     Building Pager Single-Slot Firmware & UF2    ");
    println!("==================================================");

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
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
    let signing_key = load_or_create_signing_key(&repo_root);
    let verifying_key = signing_key.verifying_key();
    println!("🔑 Signing Firmware Public Key: {:?}", verifying_key.to_bytes());

    // Construct signed message: Magic + Version + image_len + target_slot + digest
    let mut signed_msg = Vec::new();
    signed_msg.extend_from_slice(&MANIFEST_MAGIC);
    signed_msg.extend_from_slice(&1u32.to_le_bytes()); // Version 1
    signed_msg.extend_from_slice(&image_len.to_le_bytes());
    signed_msg.extend_from_slice(&0u32.to_le_bytes()); // target_slot = 0
    signed_msg.extend_from_slice(&digest_bytes);

    let signature = signing_key.sign(&signed_msg);

    let manifest = ManifestHeader {
        state: u32::MAX,
        magic: MANIFEST_MAGIC,
        version: 1,
        image_len,
        target_slot: 0,
        digest: digest_bytes,
        signature: signature.to_bytes(),
    };

    // Combine Manifest (112 bytes) + Raw Binary
    let manifest_bytes = unsafe {
        core::slice::from_raw_parts(
            &manifest as *const ManifestHeader as *const u8,
            core::mem::size_of::<ManifestHeader>(),
        )
    };

    let mut full_image = Vec::new();
    full_image.extend_from_slice(manifest_bytes);
    full_image.resize(256, 0);
    full_image.extend_from_slice(&raw_bin);

    // Generate UF2 blocks (512 bytes per block, 256 bytes payload)
    let payload_size = 256;
    let num_blocks = (full_image.len() + payload_size - 1) / payload_size;
    let mut uf2_data = Vec::new();

    for (block_no, chunk) in full_image.chunks(payload_size).enumerate() {
        let target_addr = FIRMWARE_START + (block_no * payload_size) as u32;

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

    println!("==================================================");
    println!("🎉 UF2 Firmware Build Success! Output:");
    println!("==================================================");
}

fn load_or_create_signing_key(repo_root: &std::path::Path) -> SigningKey {
    use ed25519_dalek::pkcs8::DecodePrivateKey;

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
                println!("🔑 Signed firmware using PEM Private Key from: {}", path.display());
                return key;
            } else if let Ok(key_bytes) = fs::read(&path) {
                if key_bytes.len() == 32 {
                    let key = SigningKey::from_bytes(&key_bytes.try_into().unwrap());
                    println!("🔑 Signed firmware using raw 32-byte Private Key from: {}", path.display());
                    return key;
                }
            }
            eprintln!("⚠️ Failed to parse PEM key at {}; falling back to Dev Key", path.display());
        }
    }

    println!("🔑 Signing Firmware with Dev Key: [0x42; 32]");
    SigningKey::from_bytes(&[0x42; 32])
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
        signed_msg.extend_from_slice(&0u32.to_le_bytes());
        signed_msg.extend_from_slice(&digest_bytes);

        let signature = signing_key.sign(&signed_msg);

        let manifest = ManifestHeader {
            state: u32::MAX,
            magic: MANIFEST_MAGIC,
            version: 1,
            image_len: raw_bin.len() as u32,
            target_slot: 0,
            digest: digest_bytes,
            signature: signature.to_bytes(),
        };

        assert_eq!(core::mem::size_of::<ManifestHeader>(), 120);
        assert!(verifying_key
            .verify_strict(&signed_msg, &Signature::from_bytes(&manifest.signature))
            .is_ok());
    }
}
