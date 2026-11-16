//! Commands behind the Vault screen. Passwords only travel one way: from the
//! form (or a file Rust reads itself) into the encrypted store. Nothing here
//! returns a password, an authenticator key or a decrypted value.

use std::sync::{Arc, Mutex};

use pindot_core::vault::{EditLogin, ImportPreview, ImportResult, LoginSummary, NewLogin, UseEntry, Vault};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

const MAIN: &str = "main";
/// A password export is a few hundred KB; this is far above a real one.
const MAX_FILE: u64 = 8 * 1024 * 1024;

fn main_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == MAIN { Ok(()) } else { Err("The vault can only be used from the Pindot window.".into()) }
}

/// The file chosen for import, read by Rust and held here until the user
/// finishes or cancels. It never goes to the web view.
#[derive(Default)]
pub struct ImportHold(Mutex<Option<Zeroizing<String>>>);

#[derive(Serialize)]
pub struct VaultView {
    logins: Vec<LoginSummary>,
    /// Where the key is kept, e.g. "macOS Keychain".
    place: String,
    /// Set when the vault can't be read (key missing or wrong); nothing was changed.
    problem: Option<String>,
}

fn vault(app: &AppHandle) -> Arc<Vault> {
    app.state::<Arc<Vault>>().inner().clone()
}

#[tauri::command]
pub fn vault_state(window: tauri::WebviewWindow, app: AppHandle) -> Result<VaultView, String> {
    main_only(&window)?;
    let v = vault(&app);
    let (logins, problem) = match v.list() {
        Ok(l) => (l, None),
        Err(e) => (vec![], Some(e.to_string())),
    };
    Ok(VaultView { logins, place: v.place_of_key(), problem })
}

#[tauri::command]
pub fn vault_add(window: tauri::WebviewWindow, app: AppHandle, login: NewLogin) -> Result<LoginSummary, String> {
    main_only(&window)?;
    vault(&app).add(login).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_update(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: String,
    login: EditLogin,
) -> Result<LoginSummary, String> {
    main_only(&window)?;
    vault(&app).update(&id, login).map_err(|e| e.to_string())
}

/// The web view asks the user to confirm before calling this.
#[tauri::command]
pub fn vault_delete(window: tauri::WebviewWindow, app: AppHandle, id: String) -> Result<(), String> {
    main_only(&window)?;
    vault(&app).delete(&id).map_err(|e| e.to_string())
}

/// One login's history, or (no id) the latest of all for the Activity view.
#[tauri::command]
pub fn vault_uses(window: tauri::WebviewWindow, app: AppHandle, id: Option<String>) -> Result<Vec<UseEntry>, String> {
    main_only(&window)?;
    let v = vault(&app);
    Ok(match id {
        Some(id) => v.uses_for(&id, 200),
        None => v.all_uses(500),
    })
}

/// Opens the system file picker, reads the CSV in Rust and returns what an
/// import would do. `None`: the user cancelled.
#[tauri::command]
pub async fn vault_import_choose(window: tauri::WebviewWindow, app: AppHandle) -> Result<Option<ImportPreview>, String> {
    use tauri_plugin_dialog::DialogExt;
    main_only(&window)?;
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Choose a password export (CSV)")
        .add_filter("CSV", &["csv"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let len = std::fs::metadata(&path).map_err(|e| format!("could not read that file: {}", e.kind()))?.len();
    if len > MAX_FILE {
        return Err("that file is too big to be a password export".into());
    }
    let text = Zeroizing::new(std::fs::read_to_string(&path).map_err(|_| "could not read that file as text".to_string())?);
    let preview = vault(&app).import_preview(&text).map_err(|e| e.to_string())?;
    *app.state::<ImportHold>().0.lock().unwrap() = Some(text);
    Ok(Some(preview))
}

/// Imports the chosen file. `replace` lists the saved logins the user
/// ticked to overwrite; nothing else is ever overwritten.
#[tauri::command]
pub fn vault_import_apply(window: tauri::WebviewWindow, app: AppHandle, replace: Vec<String>) -> Result<ImportResult, String> {
    main_only(&window)?;
    let text = app.state::<ImportHold>().0.lock().unwrap().take().ok_or("choose a file first")?;
    vault(&app).import_apply(&text, &replace, "app").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn vault_import_cancel(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    main_only(&window)?;
    app.state::<ImportHold>().0.lock().unwrap().take();
    Ok(())
}
