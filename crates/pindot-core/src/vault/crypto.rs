//! AES-256-GCM for vault values. Every value is sealed on its own with a
//! fresh 12-byte nonce, and the record's id, the value's purpose and (for
//! logins) the site are bound in as associated data: a value copied onto
//! another record, another purpose or another site fails to open.
//!
//! Stored form: base64 of `nonce (12) | tag (16) | ciphertext`.
//!
//! The layout follows the one n8n's Nod documents for its vault (Apache-2.0).
//! No code was copied.

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

const NONCE: usize = 12;
const TAG: usize = 16;

/// The 32-byte master key. Zeroed when dropped; never printed.
pub struct Key(Zeroizing<[u8; 32]>);

impl Key {
    pub fn generate() -> Self {
        let mut k = [0u8; 32];
        rand::rng().fill_bytes(&mut k);
        Key(Zeroizing::new(k))
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, CryptoError> {
        let arr: [u8; 32] = b.try_into().map_err(|_| CryptoError::BadKey)?;
        Ok(Key(Zeroizing::new(arr)))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Clone for Key {
    fn clone(&self) -> Self {
        Key(Zeroizing::new(*self.0))
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("the vault key is not a valid key")]
    BadKey,
    #[error("a stored value is not valid")]
    Malformed,
    #[error("a stored value could not be opened with this key")]
    CannotOpen,
}

/// The associated data: a JSON array, so no two bindings can collide.
fn aad(binding: &[&str]) -> Vec<u8> {
    serde_json::to_vec(binding).unwrap_or_default()
}

pub fn seal(key: &Key, binding: &[&str], plaintext: &[u8]) -> String {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).expect("32-byte key");
    let mut nonce = [0u8; NONCE];
    rand::rng().fill_bytes(&mut nonce);
    let mut buf = Zeroizing::new(plaintext.to_vec());
    let tag = cipher
        .encrypt_in_place_detached(Nonce::from_slice(&nonce), &aad(binding), &mut buf)
        .expect("AES-GCM encryption does not fail for in-memory buffers");
    let mut out = Vec::with_capacity(NONCE + TAG + buf.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&buf);
    STANDARD.encode(out)
}

pub fn open(key: &Key, binding: &[&str], sealed: &str) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let bytes = STANDARD.decode(sealed.trim()).map_err(|_| CryptoError::Malformed)?;
    if bytes.len() < NONCE + TAG {
        return Err(CryptoError::Malformed);
    }
    let (nonce, rest) = bytes.split_at(NONCE);
    let (tag, ct) = rest.split_at(TAG);
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).expect("32-byte key");
    let mut buf = Zeroizing::new(ct.to_vec());
    cipher
        .decrypt_in_place_detached(Nonce::from_slice(nonce), &aad(binding), &mut buf, Tag::from_slice(tag))
        .map_err(|_| {
            buf.zeroize();
            CryptoError::CannotOpen
        })?;
    Ok(buf)
}

pub fn open_string(key: &Key, binding: &[&str], sealed: &str) -> Result<Zeroizing<String>, CryptoError> {
    let bytes = open(key, binding, sealed)?;
    String::from_utf8(bytes.to_vec()).map(Zeroizing::new).map_err(|_| CryptoError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: [&str; 3] = ["pindot-vault", "rec-1", "password"];

    #[test]
    fn round_trip() {
        let k = Key::generate();
        let s = seal(&k, &B, b"hunter2-correct-horse");
        assert!(!s.contains("hunter2"));
        assert_eq!(open(&k, &B, &s).unwrap().as_slice(), b"hunter2-correct-horse");
        assert_eq!(&*open_string(&k, &B, &s).unwrap(), "hunter2-correct-horse");
    }

    #[test]
    fn two_seals_differ() {
        let k = Key::generate();
        assert_ne!(seal(&k, &B, b"same"), seal(&k, &B, b"same"));
    }

    #[test]
    fn wrong_binding_fails() {
        let k = Key::generate();
        let s = seal(&k, &B, b"secret");
        for other in [
            ["pindot-vault", "rec-2", "password"],
            ["pindot-vault", "rec-1", "totp"],
            ["other-app", "rec-1", "password"],
        ] {
            assert_eq!(open(&k, &other, &s).unwrap_err(), CryptoError::CannotOpen);
        }
        assert_eq!(open(&k, &B[..2], &s).unwrap_err(), CryptoError::CannotOpen);
    }

    #[test]
    fn wrong_key_fails() {
        let s = seal(&Key::generate(), &B, b"secret");
        assert_eq!(open(&Key::generate(), &B, &s).unwrap_err(), CryptoError::CannotOpen);
    }

    #[test]
    fn tamper_fails() {
        let k = Key::generate();
        let s = seal(&k, &B, b"secret value");
        let raw = STANDARD.decode(&s).unwrap();
        // Flip one bit in the nonce, the tag and the ciphertext in turn.
        for i in [0, NONCE, NONCE + TAG] {
            let mut t = raw.clone();
            t[i] ^= 1;
            assert_eq!(open(&k, &B, &STANDARD.encode(&t)).unwrap_err(), CryptoError::CannotOpen, "byte {i}");
        }
        // Truncated and garbage input.
        assert_eq!(open(&k, &B, &STANDARD.encode(&raw[..20])).unwrap_err(), CryptoError::Malformed);
        assert_eq!(open(&k, &B, "not base64!!").unwrap_err(), CryptoError::Malformed);
        let mut longer = raw.clone();
        longer.push(0);
        assert_eq!(open(&k, &B, &STANDARD.encode(&longer)).unwrap_err(), CryptoError::CannotOpen);
    }

    #[test]
    fn key_debug_hides_bytes() {
        assert_eq!(format!("{:?}", Key::generate()), "Key(..)");
        assert!(Key::from_bytes(&[0u8; 31]).is_err());
    }
}
