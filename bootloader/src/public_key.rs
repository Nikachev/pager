include!(concat!(env!("OUT_DIR"), "/signing_keys.rs"));

pub fn verify_signature(msg: &[u8; 48], sig_bytes: &[u8; 64]) -> bool {
    use ed25519_compact::{PublicKey, Signature};
    let signature = Signature::new(*sig_bytes);
    FIRMWARE_SIGNING_PUBLIC_KEYS
        .iter()
        .any(|key| PublicKey::new(*key).verify(msg, &signature).is_ok())
}
