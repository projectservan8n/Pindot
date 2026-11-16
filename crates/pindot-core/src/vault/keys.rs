//! Where the vault's master key lives: the operating system's credential
//! store (macOS Keychain, Windows Credential Manager), never in the vault
//! file. Tests use `MemoryKeys`.

use std::sync::Mutex;

use zeroize::Zeroizing;

use super::crypto::Key;

#[derive(Debug, Clone, thiserror::Error)]
pub enum KeyError {
    #[error("the system credential store is not available: {0}")]
    Unavailable(String),
    #[error("the vault key in the system credential store is damaged")]
    Damaged,
}

pub trait KeyProvider: Send + Sync {
    /// The key, or `None` when none was ever stored. An error means the store
    /// itself could not be reached; callers must not then create a new key.
    fn load(&self) -> Result<Option<Key>, KeyError>;
    /// Store a new key. Never called when `load` returned a key.
    fn store(&self, key: &Key) -> Result<(), KeyError>;
    /// A short name for the place, for the UI and the README.
    fn place(&self) -> String;
}

/// A key held in memory only: tests, and `PINDOT_TEST_*` runs.
#[derive(Default)]
pub struct MemoryKeys {
    key: Mutex<Option<Key>>,
    /// Make `load` fail, as an unreachable or locked Keychain would.
    pub fail: std::sync::atomic::AtomicBool,
}

impl MemoryKeys {
    pub fn with_key(key: Key) -> Self {
        Self { key: Mutex::new(Some(key)), fail: Default::default() }
    }
    pub fn clear(&self) {
        *self.key.lock().unwrap() = None;
    }
}

impl KeyProvider for MemoryKeys {
    fn load(&self) -> Result<Option<Key>, KeyError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(KeyError::Unavailable("test".into()));
        }
        Ok(self.key.lock().unwrap().clone())
    }
    fn store(&self, key: &Key) -> Result<(), KeyError> {
        *self.key.lock().unwrap() = Some(key.clone());
        Ok(())
    }
    fn place(&self) -> String {
        "memory (tests)".into()
    }
}

/// Stands in when the OS credential store can't be opened at all: every call
/// says so, and the vault fails closed (nothing is read or written).
pub struct UnavailableKeys(pub String);

impl KeyProvider for UnavailableKeys {
    fn load(&self) -> Result<Option<Key>, KeyError> {
        Err(KeyError::Unavailable(self.0.clone()))
    }
    fn store(&self, _key: &Key) -> Result<(), KeyError> {
        Err(KeyError::Unavailable(self.0.clone()))
    }
    fn place(&self) -> String {
        "unavailable".into()
    }
}

/// The OS credential store, through `keyring-core` and one native store per
/// OS (no Linux Secret Service: Pindot ships for macOS and Windows).
pub struct SystemKeys {
    entry: keyring_core::Entry,
}

/// Service name of the Keychain / Credential Manager item.
pub const SERVICE: &str = "Pindot Vault";
pub const ACCOUNT: &str = "master-key";

impl SystemKeys {
    pub fn new() -> Result<Self, KeyError> {
        Self::named(SERVICE, ACCOUNT)
    }

    /// With explicit names, so a test can use a clearly labelled item.
    pub fn named(service: &str, account: &str) -> Result<Self, KeyError> {
        use keyring_core::api::CredentialStoreApi;
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(not(any(target_os = "macos", windows)))]
        return Err(KeyError::Unavailable("this operating system has no supported credential store".into()));
        #[cfg(any(target_os = "macos", windows))]
        {
            let store = store.map_err(|e| KeyError::Unavailable(e.to_string()))?;
            let entry = store.build(service, account, None).map_err(|e| KeyError::Unavailable(e.to_string()))?;
            Ok(Self { entry })
        }
    }

    /// Remove the item (used by the one clearly named test entry).
    pub fn delete(&self) -> Result<(), KeyError> {
        self.entry.delete_credential().map_err(|e| KeyError::Unavailable(e.to_string()))
    }
}

impl KeyProvider for SystemKeys {
    fn load(&self) -> Result<Option<Key>, KeyError> {
        match self.entry.get_secret() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                Key::from_bytes(&bytes).map(Some).map_err(|_| KeyError::Damaged)
            }
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeyError::Unavailable(e.to_string())),
        }
    }
    fn store(&self, key: &Key) -> Result<(), KeyError> {
        self.entry.set_secret(key.as_bytes()).map_err(|e| KeyError::Unavailable(e.to_string()))
    }
    fn place(&self) -> String {
        if cfg!(target_os = "macos") {
            "the macOS Keychain".into()
        } else if cfg!(windows) {
            "Windows Credential Manager".into()
        } else {
            "no supported credential store".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SERVICE: &str = "Pindot Vault TEST (safe to delete)";

    /// The real store of this system (Keychain, Credential Manager), with an
    /// item of its own that is removed again. Not run by default: it writes
    /// to the user's credential store, and macOS may ask for the Keychain.
    ///   cargo test -p pindot-core --lib real_credential_store -- --ignored
    #[test]
    #[ignore = "writes one clearly named item to the real OS credential store and removes it"]
    fn a_key_comes_back_from_the_real_credential_store() {
        let keys = SystemKeys::named(TEST_SERVICE, ACCOUNT).expect("the credential store opens");
        let _ = keys.delete();
        assert!(keys.load().expect("an empty store is not an error").is_none(), "nothing stored yet");

        let key = Key::generate();
        keys.store(&key).expect("the key is stored");
        let again = SystemKeys::named(TEST_SERVICE, ACCOUNT).unwrap();
        let back = again.load().expect("the store is read").expect("the key is there");
        assert_eq!(back.as_bytes(), key.as_bytes(), "the same 32 bytes come back");

        keys.delete().expect("the test item is removed");
        assert!(again.load().unwrap().is_none(), "nothing is left behind");
    }
}
