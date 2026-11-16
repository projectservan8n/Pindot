//! Choosing a brain: the provider list, local servers, a provider's
//! models, its API key (stored, replaced or deleted, never shown) and the
//! model test.

use std::sync::Arc;

use pindot_core::brain::native::openai::{Who, describe};
use pindot_core::brain::native::probe::{self, FoundServer, ModelInfo};
use pindot_core::brain::native::providers::{self, PROVIDERS};
use pindot_core::brain::native::secrets::{SecretStore, SystemSecrets, redact};
use pindot_core::brain::native::settings::{BrainSettings, TestResult};
use pindot_core::brain::native::test_model::test_model;
use pindot_core::server::{RunningServer, ServerState};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::ui::Ui;

#[derive(Serialize)]
pub struct ProviderView {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    local: bool,
    needs_key: bool,
    /// A key for it is in the credential store (never the key itself).
    has_key: bool,
}

#[tauri::command]
pub async fn brain_providers() -> Vec<ProviderView> {
    tauri::async_runtime::spawn_blocking(|| {
        let store = SystemSecrets::default();
        PROVIDERS
            .iter()
            .map(|p| ProviderView {
                id: p.id,
                name: p.name,
                base_url: p.base_url,
                local: p.local,
                needs_key: p.needs_key,
                has_key: !p.local && store.get(p.id).ok().flatten().is_some(),
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[derive(Serialize)]
pub struct CodexStatus {
    /// Where the Codex binary is; `None` = not installed.
    path: Option<String>,
    signed_in: bool,
    /// The sign-in check itself failed.
    problem: Option<String>,
}

/// The Codex CLI: installed? signed in (`codex login status`)? Read-only.
#[tauri::command]
pub async fn brain_codex_status() -> CodexStatus {
    tauri::async_runtime::spawn_blocking(|| match pindot_core::brain::codex::find_codex() {
        None => CodexStatus { path: None, signed_in: false, problem: None },
        Some(bin) => {
            let (signed_in, problem) = match pindot_core::brain::codex::signed_in(&bin) {
                Ok(s) => (s, None),
                Err(e) => (false, Some(e)),
            };
            CodexStatus { path: Some(bin.display().to_string()), signed_in, problem }
        }
    })
    .await
    .unwrap_or(CodexStatus { path: None, signed_in: false, problem: Some("the check could not run".into()) })
}

/// Ollama, LM Studio and llama.cpp running now, with their models.
#[tauri::command]
pub async fn brain_find_local() -> Vec<FoundServer> {
    tauri::async_runtime::spawn_blocking(probe::find_local).await.unwrap_or_default()
}

#[tauri::command]
pub async fn brain_models(provider: String, base_url: String) -> Result<Vec<ModelInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = providers::find(&provider).ok_or("Unknown provider.")?;
        let key = SystemSecrets::default().get(&provider).ok().flatten();
        let who = Who { provider: p.name.into(), model: String::new(), local: p.local };
        probe::list_models(&provider, &base_url, key.as_deref()).map_err(|e| redact(&describe(&e, &who), key.as_deref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Saves the key; returns a warning when it looks like another provider's.
#[tauri::command]
pub async fn brain_key_set(provider: String, key: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = providers::find(&provider).ok_or("Unknown provider.")?;
        SystemSecrets::default().set(&provider, &key)?;
        Ok(providers::key_warning(p, &key))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn brain_key_delete(provider: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || SystemSecrets::default().delete(&provider)).await.map_err(|e| e.to_string())?
}

/// Runs the model test and records the result in settings.json.
#[tauri::command]
pub async fn brain_test(app: AppHandle, brain: BrainSettings) -> Result<TestResult, String> {
    let (Some(server), Some(state)) = (app.try_state::<RunningServer>(), app.try_state::<Arc<ServerState>>()) else {
        return Err("The MCP server is not running, so the desktop tools can't be tested.".into());
    };
    let (mcp_url, token, state) = (server.mcp_url(), server.token.get().to_string(), state.inner().clone());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let key = SystemSecrets::default().get(&brain.provider).ok().flatten();
        let r = test_model(&brain, key.clone(), &mcp_url, &token, &state);
        TestResult {
            base_url: brain.base_url,
            model: brain.model,
            ok: r.ok,
            note: redact(&r.note, key.as_deref()),
            at: crate::chat::now_secs(),
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    println!("brain test: {} on {}: {}", result.model, result.base_url, if result.ok { "passed" } else { &result.note });
    let ui = app.state::<Ui>();
    let mut s = ui.settings();
    s.brain.record_test(result.clone());
    s.save().map_err(|e| format!("Saving: {e:#}"))?;
    *ui.settings.lock().unwrap() = s;
    Ok(result)
}

/// Setup's "Use this brain": saves only the brain block and restarts the
/// assistant when the brain changed.
#[tauri::command]
pub fn brain_choose(app: AppHandle, brain: BrainSettings) -> Result<(), Vec<String>> {
    let problems = brain.problems();
    if !problems.is_empty() {
        return Err(problems);
    }
    let ui = app.state::<Ui>();
    let mut s = ui.settings();
    let restart = !s.brain.same_brain(&brain);
    let tested = std::mem::take(&mut s.brain.tested);
    s.brain = BrainSettings { tested, ..brain };
    s.save().map_err(|e| vec![format!("Saving: {e:#}")])?;
    *ui.settings.lock().unwrap() = s;
    if restart {
        crate::ui::restart_voice(&app);
    }
    crate::ui::changed(&app);
    Ok(())
}
