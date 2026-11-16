//! The vault: website logins the user saves once, which Rusty can sign in
//! with without ever receiving the secret. Pindot types it itself.
//!
//! * `crypto`: AES-256-GCM, each value bound to its record, purpose and site.
//! * `keys`: the master key, in the OS credential store.
//! * `origin`: exact scheme + host + port matching.
//! * `totp`: authenticator codes (RFC 6238).
//! * `import`: CSV exports from Chrome, 1Password and Bitwarden.
//! * `gate`: rate limit and lock, per session.
//!
//! The password and authenticator key are write-only through this API: the
//! app can add, replace and delete them, and can never read them back. The
//! only reader is `Vault::secret`, which is `pub(crate)` and used by the
//! server's `vault_fill` after its safety checks.
//!
//! The design (not the code) follows n8n's Nod vault, Apache-2.0.

pub mod crypto;
pub mod gate;
pub mod import;
pub mod keys;
pub mod origin;
pub mod totp;

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::server::activity::utc_now;
use crypto::{CryptoError, Key};
use keys::{KeyError, KeyProvider};

const APP: &str = "pindot-vault";
const FILE: &str = "vault.json";
const USES: &str = "vault-uses.jsonl";
const USES_MAX_BYTES: u64 = 2 * 1024 * 1024;
const USES_KEEP: usize = 2000;
const MAX_FIELD: usize = 1024;
const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VaultError {
    #[error("no such saved login")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Origin(#[from] origin::OriginError),
    #[error("{0}")]
    Totp(#[from] totp::TotpError),
    #[error("{0}")]
    Import(#[from] import::ImportError),
    #[error("the vault key could not be read from {0}; nothing was changed")]
    KeyUnavailable(String),
    #[error("the vault has saved logins but its key is missing from the system credential store; nothing was changed")]
    KeyMissing,
    #[error("the key in the system credential store does not open this vault; nothing was changed")]
    WrongKey,
    #[error("this login can't be opened with the current key (it can still be deleted)")]
    CannotOpen,
    #[error("the vault file could not be read or written: {0}")]
    Io(String),
}

impl From<KeyError> for VaultError {
    fn from(e: KeyError) -> Self {
        VaultError::KeyUnavailable(e.to_string())
    }
}

impl From<CryptoError> for VaultError {
    fn from(_: CryptoError) -> Self {
        VaultError::CannotOpen
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Record {
    id: String,
    name: String,
    origin: String,
    username: String,
    password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    totp: Option<String>,
    created: String,
    updated: String,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    version: u32,
    #[serde(default)]
    check: Option<String>,
    #[serde(default)]
    logins: Vec<Record>,
}

/// What the app and the model may see of a login. Never the password or key.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoginSummary {
    pub id: String,
    pub name: String,
    pub origin: String,
    /// Empty when `locked`.
    pub username: String,
    pub has_2fa: bool,
    pub created: String,
    pub updated: String,
    pub last_used: Option<String>,
    /// The current key can't open this login.
    pub locked: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewLogin {
    pub name: String,
    pub site: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub authenticator: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Leave as it is.
    #[default]
    Keep,
    Clear,
    Set(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditLogin {
    pub name: String,
    pub site: String,
    pub username: String,
    /// Empty or absent: keep the saved password.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub authenticator: Change,
}

/// Which secret `vault_fill` types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Username,
    Password,
    Code,
}

impl Field {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "username" | "user" | "email" => Some(Field::Username),
            "password" => Some(Field::Password),
            "code" | "otp" | "totp" | "authenticator" | "2fa" => Some(Field::Code),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Field::Username => "username",
            Field::Password => "password",
            Field::Code => "code",
        }
    }
}

/// One line of the use log. No secret values, ever.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UseEntry {
    pub ts: String,
    pub login_id: String,
    pub login_name: String,
    pub site: String,
    /// "username", "password", "code", or "-" for a change to the vault.
    pub field: String,
    /// "typed", "refused", "added", "changed", "deleted", "imported".
    pub result: String,
    /// Why, in words (never a value).
    pub detail: String,
    pub session: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub id: String,
    pub name: String,
    pub site: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub format: String,
    pub columns: Vec<import::Column>,
    pub total_rows: usize,
    /// Logins that would be added.
    pub new_logins: usize,
    /// Already saved with the same password.
    pub unchanged: usize,
    /// Already saved with a different password: only replaced if asked.
    pub conflicts: Vec<Conflict>,
    /// Saved logins that would only gain an authenticator key.
    pub gain_key: usize,
    pub skipped: Vec<import::Skip>,
    pub bad_authenticator: usize,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct ImportResult {
    pub imported: usize,
    pub replaced: usize,
    pub keys_added: usize,
    pub unchanged: usize,
    /// Different password, not replaced.
    pub kept_existing: usize,
    pub skipped: usize,
    pub skipped_reasons: Vec<import::Skip>,
}

pub struct Vault {
    dir: PathBuf,
    keys: Arc<dyn KeyProvider>,
    key: Mutex<Option<Key>>,
    /// Serialises read-modify-write of the file.
    io: Mutex<()>,
}

fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 12];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn clean(s: &str, what: &str, required: bool) -> Result<String, VaultError> {
    let t = s.trim();
    if required && t.is_empty() {
        return Err(VaultError::Invalid(format!("{what} is required")));
    }
    if t.len() > MAX_FIELD || t.chars().any(|c| c == '\0') {
        return Err(VaultError::Invalid(format!("{what} is too long or has bad characters")));
    }
    Ok(t.to_string())
}

impl Vault {
    pub fn new(dir: PathBuf, keys: Arc<dyn KeyProvider>) -> Self {
        Vault { dir, keys, key: Mutex::new(None), io: Mutex::new(()) }
    }

    pub fn place_of_key(&self) -> String {
        self.keys.place()
    }

    fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    fn read(&self) -> Result<File, VaultError> {
        match fs::read_to_string(self.path()) {
            Ok(t) => serde_json::from_str(&t).map_err(|_| VaultError::Io("the vault file is damaged".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File { version: 1, ..Default::default() }),
            Err(e) => Err(VaultError::Io(e.kind().to_string())),
        }
    }

    fn write(&self, f: &File) -> Result<(), VaultError> {
        let bytes = serde_json::to_vec_pretty(f).map_err(|e| VaultError::Io(e.to_string()))?;
        crate::paths::ensure_private_dir(&self.dir).map_err(|e| VaultError::Io(e.to_string()))?;
        crate::paths::write_private(&self.path(), &bytes).map_err(|e| VaultError::Io(e.to_string()))
    }

    fn bind<'a>(id: &'a str, purpose: &'a str, origin: &'a str) -> [&'a str; 4] {
        [APP, id, purpose, origin]
    }

    /// The key, checked against what is already saved. `create`: make one if
    /// there is none and nothing is saved yet. Fails closed: an unreachable
    /// store, or a missing/wrong key for a vault that has data, stops here.
    fn key(&self, file: &File, create: bool) -> Result<Option<Key>, VaultError> {
        if let Some(k) = self.key.lock().unwrap().clone() {
            return Ok(Some(k));
        }
        let has_data = file.check.is_some() || !file.logins.is_empty();
        match self.keys.load()? {
            Some(k) => {
                if let Some(c) = &file.check {
                    if crypto::open(&k, &[APP, "vault", "check"], c).is_err() {
                        return Err(VaultError::WrongKey);
                    }
                } else if let Some(r) = file.logins.first() {
                    if crypto::open(&k, &Self::bind(&r.id, "password", &r.origin), &r.password).is_err() {
                        return Err(VaultError::WrongKey);
                    }
                }
                *self.key.lock().unwrap() = Some(k.clone());
                Ok(Some(k))
            }
            None if has_data => Err(VaultError::KeyMissing),
            None if !create => Ok(None),
            None => {
                let k = Key::generate();
                self.keys.store(&k)?;
                // Read it back: if the store did not keep it, don't write data
                // that nothing could open.
                if self.keys.load()?.is_none() {
                    return Err(VaultError::KeyUnavailable(self.keys.place()));
                }
                *self.key.lock().unwrap() = Some(k.clone());
                Ok(Some(k))
            }
        }
    }

    fn ensure_check(&self, file: &mut File, key: &Key) {
        if file.check.is_none() {
            file.check = Some(crypto::seal(key, &[APP, "vault", "check"], b"ok"));
        }
        file.version = 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn seal_record(
        key: &Key,
        id: &str,
        name: &str,
        origin: &str,
        username: &str,
        password: &str,
        totp: Option<&str>,
        created: &str,
    ) -> Record {
        Record {
            id: id.into(),
            name: name.into(),
            origin: origin.into(),
            username: crypto::seal(key, &Self::bind(id, "username", origin), username.as_bytes()),
            password: crypto::seal(key, &Self::bind(id, "password", origin), password.as_bytes()),
            totp: totp.map(|t| crypto::seal(key, &Self::bind(id, "totp", origin), t.as_bytes())),
            created: created.into(),
            updated: utc_now(),
        }
    }

    fn summary(&self, key: Option<&Key>, r: &Record, last: &HashMap<String, String>) -> LoginSummary {
        let username = key
            .and_then(|k| crypto::open_string(k, &Self::bind(&r.id, "username", &r.origin), &r.username).ok())
            .map(|u| u.to_string());
        LoginSummary {
            id: r.id.clone(),
            name: r.name.clone(),
            origin: r.origin.clone(),
            locked: username.is_none(),
            username: username.unwrap_or_default(),
            has_2fa: r.totp.is_some(),
            created: r.created.clone(),
            updated: r.updated.clone(),
            last_used: last.get(&r.id).cloned(),
        }
    }

    fn last_used(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for u in self.all_uses(usize::MAX) {
            if u.ok && u.result == "typed" {
                m.insert(u.login_id, u.ts);
            }
        }
        m
    }

    // ---- reading (no secrets) ------------------------------------------

    pub fn list(&self) -> Result<Vec<LoginSummary>, VaultError> {
        let _g = self.io.lock().unwrap();
        let file = self.read()?;
        if file.logins.is_empty() {
            return Ok(vec![]);
        }
        let key = self.key(&file, false)?;
        let last = self.last_used();
        let mut out: Vec<_> = file.logins.iter().map(|r| self.summary(key.as_ref(), r, &last)).collect();
        out.sort_by_key(|l| l.name.to_lowercase());
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<LoginSummary, VaultError> {
        self.list()?.into_iter().find(|l| l.id == id).ok_or(VaultError::NotFound)
    }

    /// Logins saved for exactly this origin: id, name, username hint.
    pub fn for_origin(&self, origin: &str) -> Result<Vec<LoginSummary>, VaultError> {
        let want = origin::parse_saved_origin(origin)?;
        Ok(self.list()?.into_iter().filter(|l| l.origin == want).collect())
    }

    /// Does the vault hold any login at all? (No key needed.)
    pub fn is_empty(&self) -> bool {
        self.read().map(|f| f.logins.is_empty()).unwrap_or(true)
    }

    // ---- changing (write-only secrets) ---------------------------------

    pub fn add(&self, n: NewLogin) -> Result<LoginSummary, VaultError> {
        let name = clean(&n.name, "name", true)?;
        let origin = origin::parse_saved_origin(&n.site)?;
        let username = clean(&n.username, "user name", false)?;
        if n.password.is_empty() || n.password.len() > MAX_FIELD {
            return Err(VaultError::Invalid("password is required".into()));
        }
        let totp = n.authenticator.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if let Some(t) = totp {
            totp::Totp::parse(t)?;
        }
        let _g = self.io.lock().unwrap();
        let mut file = self.read()?;
        let key = self.key(&file, true)?.expect("created");
        self.ensure_check(&mut file, &key);
        let id = new_id();
        let now = utc_now();
        file.logins.push(Self::seal_record(&key, &id, &name, &origin, &username, &n.password, totp, &now));
        self.write(&file)?;
        drop(_g);
        self.log_change(&id, &name, &origin, "added", "");
        self.get(&id)
    }

    pub fn update(&self, id: &str, e: EditLogin) -> Result<LoginSummary, VaultError> {
        let name = clean(&e.name, "name", true)?;
        let origin = origin::parse_saved_origin(&e.site)?;
        let username = clean(&e.username, "user name", false)?;
        if let Change::Set(t) = &e.authenticator {
            totp::Totp::parse(t)?;
        }
        let _g = self.io.lock().unwrap();
        let mut file = self.read()?;
        let key = self.key(&file, false)?.ok_or(VaultError::KeyMissing)?;
        let pos = file.logins.iter().position(|r| r.id == id).ok_or(VaultError::NotFound)?;
        let old = file.logins[pos].clone();
        // The old values are needed to re-seal under a changed site.
        let old_pass = match e.password.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => Zeroizing::new(p.to_string()),
            None => crypto::open_string(&key, &Self::bind(id, "password", &old.origin), &old.password)?,
        };
        let totp: Option<Zeroizing<String>> = match &e.authenticator {
            Change::Keep => match &old.totp {
                Some(t) => Some(crypto::open_string(&key, &Self::bind(id, "totp", &old.origin), t)?),
                None => None,
            },
            Change::Clear => None,
            Change::Set(t) => Some(Zeroizing::new(t.trim().to_string())),
        };
        file.logins[pos] =
            Self::seal_record(&key, id, &name, &origin, &username, &old_pass, totp.as_deref().map(String::as_str), &old.created);
        self.write(&file)?;
        drop(_g);
        self.log_change(id, &name, &origin, "changed", "");
        self.get(id)
    }

    pub fn delete(&self, id: &str) -> Result<(), VaultError> {
        let _g = self.io.lock().unwrap();
        let mut file = self.read()?;
        let pos = file.logins.iter().position(|r| r.id == id).ok_or(VaultError::NotFound)?;
        let r = file.logins.remove(pos);
        self.write(&file)?;
        drop(_g);
        self.log_change(id, &r.name, &r.origin, "deleted", "");
        Ok(())
    }

    // ---- the one reader of secrets -------------------------------------

    /// The value to type. Only the server's `vault_fill` calls this, after
    /// its checks; the app crate cannot (it is `pub(crate)`).
    pub(crate) fn secret(&self, id: &str, field: Field) -> Result<Zeroizing<String>, VaultError> {
        let _g = self.io.lock().unwrap();
        let file = self.read()?;
        let key = self.key(&file, false)?.ok_or(VaultError::KeyMissing)?;
        let r = file.logins.iter().find(|r| r.id == id).ok_or(VaultError::NotFound)?;
        match field {
            Field::Username => Ok(crypto::open_string(&key, &Self::bind(id, "username", &r.origin), &r.username)?),
            Field::Password => Ok(crypto::open_string(&key, &Self::bind(id, "password", &r.origin), &r.password)?),
            Field::Code => {
                let sealed = r.totp.as_ref().ok_or_else(|| VaultError::Invalid("this login has no authenticator key".into()))?;
                let setup = crypto::open_string(&key, &Self::bind(id, "totp", &r.origin), sealed)?;
                let t = totp::Totp::parse(&setup)?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Ok(t.code_at(now))
            }
        }
    }

    /// Does this login have a value for the field?
    pub fn has_field(&self, id: &str, field: Field) -> Result<bool, VaultError> {
        let l = self.get(id)?;
        Ok(match field {
            Field::Code => l.has_2fa,
            _ => true,
        })
    }

    // ---- import ---------------------------------------------------------

    fn existing_index(&self, file: &File, key: Option<&Key>) -> HashMap<(String, String), usize> {
        let mut m = HashMap::new();
        if let Some(k) = key {
            for (i, r) in file.logins.iter().enumerate() {
                if let Ok(u) = crypto::open_string(k, &Self::bind(&r.id, "username", &r.origin), &r.username) {
                    m.insert((r.origin.clone(), u.to_lowercase()), i);
                }
            }
        }
        m
    }

    pub fn import_preview(&self, csv: &str) -> Result<ImportPreview, VaultError> {
        if csv.len() > MAX_IMPORT_BYTES {
            return Err(import::ImportError::TooBig.into());
        }
        let parsed = import::parse_csv(csv)?;
        let _g = self.io.lock().unwrap();
        let file = self.read()?;
        let key = if file.logins.is_empty() { None } else { self.key(&file, false)? };
        let index = self.existing_index(&file, key.as_ref());
        let mut p = ImportPreview {
            format: parsed.format.clone(),
            columns: parsed.columns.clone(),
            total_rows: parsed.rows.len() + parsed.skipped.len(),
            new_logins: 0,
            unchanged: 0,
            conflicts: vec![],
            gain_key: 0,
            skipped: parsed.skipped.clone(),
            bad_authenticator: parsed.bad_totp,
        };
        let mut seen = std::collections::HashSet::new();
        for row in &parsed.rows {
            let k = (row.origin.clone(), row.username.to_lowercase());
            if !seen.insert(k.clone()) {
                p.skipped.push(import::Skip { line: row.line, reason: "duplicate of an earlier row".into() });
                continue;
            }
            match (index.get(&k), key.as_ref()) {
                (None, _) => p.new_logins += 1,
                (Some(&i), Some(key)) => {
                    let r = &file.logins[i];
                    let same = crypto::open_string(key, &Self::bind(&r.id, "password", &r.origin), &r.password)
                        .is_ok_and(|old| *old == *row.password);
                    if same {
                        if r.totp.is_none() && row.totp.is_some() {
                            p.gain_key += 1;
                        } else {
                            p.unchanged += 1;
                        }
                    } else {
                        p.conflicts.push(Conflict {
                            id: r.id.clone(),
                            name: r.name.clone(),
                            site: r.origin.clone(),
                            username: row.username.clone(),
                        });
                        if r.totp.is_none() && row.totp.is_some() {
                            p.gain_key += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(p)
    }

    /// Add what is new. A login already saved keeps its password unless its
    /// id is in `replace` (the user said yes to that one). A saved login
    /// without an authenticator key gains the file's, which replaces nothing.
    pub fn import_apply(&self, csv: &str, replace: &[String], session: &str) -> Result<ImportResult, VaultError> {
        if csv.len() > MAX_IMPORT_BYTES {
            return Err(import::ImportError::TooBig.into());
        }
        let parsed = import::parse_csv(csv)?;
        let _g = self.io.lock().unwrap();
        let mut file = self.read()?;
        let key = self.key(&file, true)?.expect("created");
        self.ensure_check(&mut file, &key);
        let mut index = self.existing_index(&file, Some(&key));
        let mut res = ImportResult { skipped: parsed.skipped.len(), skipped_reasons: parsed.skipped.clone(), ..Default::default() };
        for row in &parsed.rows {
            let k = (row.origin.clone(), row.username.to_lowercase());
            match index.get(&k).copied() {
                None => {
                    let id = new_id();
                    let rec = Self::seal_record(
                        &key,
                        &id,
                        &row.name,
                        &row.origin,
                        &row.username,
                        &row.password,
                        row.totp.as_deref().map(String::as_str),
                        &utc_now(),
                    );
                    file.logins.push(rec);
                    index.insert(k, file.logins.len() - 1);
                    res.imported += 1;
                }
                Some(i) => {
                    let r = file.logins[i].clone();
                    let same = crypto::open_string(&key, &Self::bind(&r.id, "password", &r.origin), &r.password)
                        .is_ok_and(|old| *old == *row.password);
                    let may_replace = !same && replace.contains(&r.id);
                    let add_key = r.totp.is_none() && row.totp.is_some();
                    if may_replace || add_key {
                        let pass = if may_replace {
                            row.password.clone()
                        } else {
                            crypto::open_string(&key, &Self::bind(&r.id, "password", &r.origin), &r.password)?
                        };
                        let totp = if add_key {
                            row.totp.as_deref().cloned()
                        } else {
                            match &r.totp {
                                Some(t) => Some(crypto::open_string(&key, &Self::bind(&r.id, "totp", &r.origin), t)?.to_string()),
                                None => None,
                            }
                        };
                        let username = crypto::open_string(&key, &Self::bind(&r.id, "username", &r.origin), &r.username)?;
                        file.logins[i] = Self::seal_record(
                            &key,
                            &r.id,
                            &r.name,
                            &r.origin,
                            &username,
                            &pass,
                            totp.as_deref(),
                            &r.created,
                        );
                        if may_replace {
                            res.replaced += 1;
                        } else if !same {
                            res.kept_existing += 1;
                        }
                        if add_key {
                            res.keys_added += 1;
                        }
                    } else if same {
                        res.unchanged += 1;
                    } else {
                        res.kept_existing += 1;
                    }
                }
            }
        }
        self.write(&file)?;
        drop(_g);
        self.append_use(&UseEntry {
            ts: utc_now(),
            login_id: String::new(),
            login_name: "(import)".into(),
            site: String::new(),
            field: "-".into(),
            result: "imported".into(),
            detail: format!(
                "{} added, {} replaced, {} skipped, {} kept as they were",
                res.imported, res.replaced, res.skipped, res.kept_existing
            ),
            session: session.into(),
            ok: true,
        });
        Ok(res)
    }

    // ---- the use log ----------------------------------------------------

    fn log_change(&self, id: &str, name: &str, origin: &str, what: &str, detail: &str) {
        self.append_use(&UseEntry {
            ts: utc_now(),
            login_id: id.into(),
            login_name: name.into(),
            site: origin.into(),
            field: "-".into(),
            result: what.into(),
            detail: detail.into(),
            session: "app".into(),
            ok: true,
        });
    }

    pub fn append_use(&self, e: &UseEntry) {
        let path = self.dir.join(USES);
        let Ok(line) = serde_json::to_string(e) else { return };
        if crate::paths::ensure_private_dir(&self.dir).is_err() {
            return;
        }
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(mut f) => {
                if writeln!(f, "{line}").is_err() {
                    tracing::warn!("vault use log write failed");
                }
            }
            Err(_) => tracing::warn!("vault use log could not be opened"),
        }
        trim_uses(&path);
    }

    /// The newest `limit` entries, oldest first.
    pub fn all_uses(&self, limit: usize) -> Vec<UseEntry> {
        let Ok(text) = fs::read_to_string(self.dir.join(USES)) else { return vec![] };
        let all: Vec<UseEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let skip = all.len().saturating_sub(limit);
        all[skip..].to_vec()
    }

    pub fn uses_for(&self, id: &str, limit: usize) -> Vec<UseEntry> {
        let mut v: Vec<_> = self.all_uses(usize::MAX).into_iter().filter(|u| u.login_id == id).collect();
        let skip = v.len().saturating_sub(limit);
        v.drain(..skip);
        v
    }
}

fn trim_uses(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= USES_MAX_BYTES {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else { return };
    let lines: Vec<&str> = text.lines().collect();
    let keep = lines[lines.len().saturating_sub(USES_KEEP)..].join("\n") + "\n";
    let _ = crate::paths::write_private(path, keep.as_bytes());
}

#[cfg(test)]
mod tests;
