//! API keys for the loop's providers, in the OS credential store (the
//! macOS Keychain, Windows Credential Manager). Never in settings.json, a
//! log, an error message or a conversation file.

use std::collections::HashMap;
use std::sync::Mutex;

pub const SERVICE: &str = "Pindot AI keys";

pub trait SecretStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<String>, String>;
    fn set(&self, provider: &str, key: &str) -> Result<(), String>;
    fn delete(&self, provider: &str) -> Result<(), String>;
}

/// Tests only.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(provider).cloned())
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let key = checked(key)?;
        self.0.lock().unwrap().insert(provider.into(), key);
        Ok(())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(provider);
        Ok(())
    }
}

fn checked(key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("The key is empty.".into());
    }
    if k.chars().any(char::is_whitespace) || k.len() > 512 {
        return Err("That doesn't look like an API key.".into());
    }
    Ok(k.to_string())
}

pub struct SystemSecrets {
    service: String,
}

impl Default for SystemSecrets {
    fn default() -> Self {
        Self::named(SERVICE)
    }
}

impl SystemSecrets {
    pub fn named(service: &str) -> Self {
        Self { service: service.into() }
    }

    fn entry(&self, provider: &str) -> Result<keyring_core::Entry, String> {
        use keyring_core::api::CredentialStoreApi;
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(not(any(target_os = "macos", windows)))]
        return Err("this operating system has no supported credential store".into());
        #[cfg(any(target_os = "macos", windows))]
        {
            let store = store.map_err(|e| e.to_string())?;
            store.build(&self.service, provider, None).map_err(|e| e.to_string())
        }
    }
}

impl SecretStore for SystemSecrets {
    fn get(&self, provider: &str) -> Result<Option<String>, String> {
        match self.entry(provider)?.get_secret() {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| "the saved key is damaged".into()),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, provider: &str, key: &str) -> Result<(), String> {
        let key = checked(key)?;
        self.entry(provider)?.set_secret(key.as_bytes()).map_err(|e| e.to_string())
    }
    fn delete(&self, provider: &str) -> Result<(), String> {
        match self.entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// `text` with every copy of `key` replaced, for anything that might be
/// shown or logged.
pub fn redact(text: &str, key: Option<&str>) -> String {
    match key.map(str::trim).filter(|k| k.len() >= 8) {
        Some(k) => text.replace(k, "[key]"),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_secrets_round_trip() {
        let s = MemorySecrets::default();
        assert_eq!(s.get("openai").unwrap(), None);
        s.set("openai", "  sk-test  ").unwrap();
        assert_eq!(s.get("openai").unwrap().as_deref(), Some("sk-test"));
        assert!(s.set("openai", "   ").is_err());
        assert!(s.set("openai", "two words").is_err());
        s.delete("openai").unwrap();
        s.delete("openai").unwrap();
        assert_eq!(s.get("openai").unwrap(), None);
    }

    #[test]
    fn a_key_is_scrubbed_from_text() {
        assert_eq!(redact("bad key sk-abc123 rejected", Some("sk-abc123")), "bad key [key] rejected");
        assert_eq!(redact("nothing", None), "nothing");
        assert_eq!(redact("short ab", Some("ab")), "short ab", "keys under 8 characters are not real keys");
    }

    /// Writes to the real Keychain / Credential Manager under a test name.
    ///   cargo test -p pindot-core --lib real_secret_store -- --ignored
    #[test]
    #[ignore]
    fn real_secret_store() {
        let s = SystemSecrets::named("Pindot AI keys TEST (safe to delete)");
        s.set("probe", "sk-real-store-test").unwrap();
        assert_eq!(s.get("probe").unwrap().as_deref(), Some("sk-real-store-test"));
        s.delete("probe").unwrap();
        assert_eq!(s.get("probe").unwrap(), None);
    }
}
