#[cfg(test)]
use super::*;
use ed25519_dalek::Signature;
use pager_bootloader_core::codec::{NRF52840_FAMILY_ID, UF2_MAGIC_END};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "pager-xtask-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Pager test")
            .env("GIT_AUTHOR_EMAIL", "pager@example.invalid")
            .env("GIT_COMMITTER_NAME", "Pager test")
            .env("GIT_COMMITTER_EMAIL", "pager@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn raw_non_utf8_and_pem_keys_parse_and_invalid_keys_fail() {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let raw = [0xff; 32];
    let key = parse_signing_key(&raw).unwrap();
    assert_eq!(
        key.verifying_key(),
        SigningKey::from_bytes(&raw).verifying_key()
    );
    let pem = key.to_pkcs8_pem(pkcs8::LineEnding::LF).unwrap();
    assert_eq!(
        parse_signing_key(pem.as_bytes()).unwrap().verifying_key(),
        key.verifying_key()
    );
    assert!(parse_signing_key(b"invalid key").is_err());
    assert!(parse_signing_key(&[0xff; 31]).is_err());
}

#[test]
fn release_requires_clean_main_and_matching_public_key() {
    let root = TestDirectory::new();
    root.git(&["init", "-b", "main"]);
    fs::write(root.0.join("README"), b"fixture").unwrap();
    root.git(&["add", "README"]);
    root.git(&["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
    assert_release_tree(&root.0).unwrap();
    fs::write(root.0.join("untracked"), b"dirty").unwrap();
    assert!(assert_release_tree(&root.0).is_err());
    fs::remove_file(root.0.join("untracked")).unwrap();
    root.git(&["checkout", "-b", "feature"]);
    assert!(assert_release_tree(&root.0).is_err());
    fs::create_dir(root.0.join("bootloader")).unwrap();
    let key = SigningKey::from_bytes(&[0x42; 32]);
    fs::write(
        root.0.join("bootloader/firmware_signing_public.hex"),
        hex(&key.verifying_key().to_bytes()),
    )
    .unwrap();
    assert_release_signing_key(&root.0, &key).unwrap();
    let wrong = SigningKey::from_bytes(&[0x43; 32]);
    assert!(assert_release_signing_key(&root.0, &wrong).is_err());
}

#[test]
fn package_publication_preserves_old_snapshot_and_isolates_board_mode() {
    let root = TestDirectory::new();
    let stage = staging_dir(&root.0, "xiao-nrf52840", false, "app");
    fs::write(stage.join("image"), b"first").unwrap();
    let published = publish_package(&stage, "app");
    let old = fs::canonicalize(&published).unwrap();
    let stage = staging_dir(&root.0, "xiao-nrf52840", false, "app");
    fs::write(stage.join("image"), b"second").unwrap();
    assert_eq!(fs::read(published.join("image")).unwrap(), b"first");
    publish_package(&stage, "app");
    assert_eq!(fs::read(published.join("image")).unwrap(), b"second");
    assert_eq!(fs::read(old.join("image")).unwrap(), b"first");
    assert_ne!(
        published,
        artifact_dir(&root.0, "nice-nano-v2", false, "app")
    );
    assert_ne!(
        published,
        artifact_dir(&root.0, "xiao-nrf52840", true, "app")
    );
}

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

#[test]
fn xiao_installer_pads_final_payload_for_factory_bootloader() {
    let image = vec![0xA5; 300];
    let uf2 = encode_xiao_installer_uf2(&image);
    assert_eq!(uf2.len(), 1024);
    for block in uf2.chunks_exact(512) {
        assert_eq!(u32::from_le_bytes(block[16..20].try_into().unwrap()), 256);
    }
    assert_eq!(&uf2[512 + 32..512 + 32 + 44], &image[256..]);
    assert!(uf2[512 + 32 + 44..512 + 32 + 256]
        .iter()
        .all(|b| *b == 0xFF));
}

#[test]
fn uf2_encoder_uses_requested_base_and_nrf52840_family() {
    let image = vec![0xA5; 300];
    let uf2 = encode_uf2(&image, XIAO_FACTORY_APPLICATION_START);
    assert_eq!(uf2.len(), 1024);
    for (number, block) in uf2.chunks_exact(512).enumerate() {
        assert_eq!(
            u32::from_le_bytes(block[12..16].try_into().unwrap()),
            XIAO_FACTORY_APPLICATION_START + number as u32 * 256
        );
        assert_eq!(
            u32::from_le_bytes(block[28..32].try_into().unwrap()),
            NRF52840_FAMILY_ID
        );
        assert_eq!(
            u32::from_le_bytes(block[508..512].try_into().unwrap()),
            UF2_MAGIC_END
        );
    }
    assert_eq!(&uf2[32..32 + 256], &image[..256]);
    assert_eq!(&uf2[512 + 32..512 + 32 + 44], &image[256..]);
}
#[test]
fn source_identity_tracks_changes_deletion_and_untracked_files_but_excludes_keys() {
    let root = TestDirectory::new();
    root.git(&["init", "-b", "main"]);
    fs::write(root.0.join("source.rs"), b"initial source").unwrap();
    root.git(&["add", "source.rs"]);
    let initial = source_fingerprint(&root.0).unwrap();
    fs::create_dir(root.0.join("keys")).unwrap();
    fs::write(root.0.join("keys/test-key"), b"excluded local material").unwrap();
    assert_eq!(initial, source_fingerprint(&root.0).unwrap());
    fs::write(root.0.join("source.rs"), b"changed source").unwrap();
    let changed = source_fingerprint(&root.0).unwrap();
    assert_ne!(initial, changed);
    fs::write(root.0.join("new.rs"), b"untracked source").unwrap();
    assert_ne!(changed, source_fingerprint(&root.0).unwrap());
    fs::remove_file(root.0.join("new.rs")).unwrap();
    assert_eq!(changed, source_fingerprint(&root.0).unwrap());
    fs::remove_file(root.0.join("source.rs")).unwrap();
    assert_ne!(initial, source_fingerprint(&root.0).unwrap());
}
