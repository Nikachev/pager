use super::*;

pub(super) fn decode_hex_32(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub(super) fn parse_signing_key(bytes: &[u8]) -> Result<SigningKey, String> {
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    if bytes.len() == 32 {
        return Ok(SigningKey::from_bytes(bytes.try_into().unwrap()));
    }
    let pem = std::str::from_utf8(bytes)
        .map_err(|_| "signing key must be raw 32-byte or PEM".to_owned())?;
    SigningKey::from_pkcs8_pem(pem).map_err(|_| "invalid PEM signing key".to_owned())
}

pub(super) fn load_signing_key(
    repo_root: &std::path::Path,
    release: bool,
) -> Result<SigningKey, String> {
    if !release {
        return Ok(ensure_dev_signing_key(repo_root));
    }
    let path = env::var_os("PAGER_SIGNING_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("keys/firmware_signing_private.pem"));
    let bytes = fs::read(&path).map_err(|_| "release signing requires readable PAGER_SIGNING_KEY or keys/firmware_signing_private.pem".to_owned())?;
    let key = parse_signing_key(&bytes)?;
    assert_release_signing_key(repo_root, &key)?;
    Ok(key)
}

pub(super) fn assert_release_signing_key(
    repo_root: &std::path::Path,
    key: &SigningKey,
) -> Result<(), String> {
    let expected = fs::read_to_string(repo_root.join("bootloader/firmware_signing_public.hex"))
        .map_err(|error| error.to_string())?;
    if decode_hex_32(&expected) != Some(key.verifying_key().to_bytes()) {
        return Err("release private key does not match bootloader public key".into());
    }
    Ok(())
}

pub(super) fn ensure_dev_signing_key(repo_root: &std::path::Path) -> SigningKey {
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
