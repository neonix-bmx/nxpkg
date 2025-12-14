// src/trust.rs
// Minimal Ed25519 signature verification for repository index authenticity.

use ed25519_dalek::{Signature, VerifyingKey};

pub fn verify_ed25519_index(index_bytes: &[u8], sig_bytes: &[u8], pubkey_bytes: &[u8]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(pubkey_bytes.try_into().unwrap_or(&[0u8; 32])) else { return false };
    let Ok(sig) = Signature::from_slice(sig_bytes) else { return false };
    vk.verify_strict(index_bytes, &sig).is_ok()
}
