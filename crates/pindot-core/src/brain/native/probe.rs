//! What the loop can learn from a server: which local servers are running,
//! which models a server has, and what a model can do (see pictures, use
//! tools, how much context). Each server says it its own way; anything
//! unknown is `None`. Read each API's current documentation before
//! changing a field name.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::openai::{CallError, Endpoint};
use super::providers::{self, PROVIDERS};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub vision: Option<bool>,
    pub tools: Option<bool>,
    pub context: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FoundServer {
    pub provider: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<ModelInfo>,
}

/// Ollama runs a model with a short context unless told otherwise
/// (`OLLAMA_CONTEXT_LENGTH`); a context Pindot cannot know is taken as this.
pub const OLLAMA_DEFAULT_CONTEXT: u32 = 8192;
/// Everything else, when the server does not say.
pub const DEFAULT_CONTEXT: u32 = 32_768;

/// The server's root (the base URL without its trailing `/v1`).
fn root_of(base_url: &str) -> String {
    let b = base_url.trim_end_matches('/');
    b.strip_suffix("/v1").unwrap_or(b).to_string()
}

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64().map(|c| c.min(u32::MAX as u64) as u32)
}

fn get_json(agent: &ureq::Agent, url: &str, key: Option<&str>) -> Result<Value, CallError> {
    let mut r = agent.get(url);
    if let Some(k) = key {
        r = r.header("Authorization", format!("Bearer {k}"));
    }
    let resp = r.call().map_err(|e| CallError::Unreachable(e.to_string()))?;
    let code = resp.status().as_u16();
    let body = resp.into_body().with_config().limit(16 * 1024 * 1024).read_to_string().unwrap_or_default();
    if code >= 400 {
        return Err(CallError::Status { code, body: body.chars().take(2000).collect() });
    }
    serde_json::from_str(&body).map_err(|e| CallError::Stream(format!("the model list was not understood: {e}")))
}

fn post_json(agent: &ureq::Agent, url: &str, body: &Value) -> Option<Value> {
    let resp = agent.post(url).header("Content-Type", "application/json").send(&body.to_string()).ok()?;
    if resp.status().as_u16() >= 400 {
        return None;
    }
    serde_json::from_str(&resp.into_body().read_to_string().ok()?).ok()
}

/// The models `provider` at `base_url` offers, with what is known about
/// each. Embedding-only models are left out.
pub fn list_models(provider: &str, base_url: &str, key: Option<&str>) -> Result<Vec<ModelInfo>, CallError> {
    if key.is_some() && providers::plain_http_remote(base_url) {
        return Err(CallError::Unreachable("a key is never sent over plain http to another computer".into()));
    }
    let agent = Endpoint::agent(Duration::from_secs(3));
    let ep = Endpoint { base_url: base_url.into(), key: key.map(str::to_string) };
    let list = get_json(&agent, &ep.url("models"), key)?;
    let mut out: Vec<ModelInfo> = vec![];
    for m in list.get("data").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = m.get("id").and_then(Value::as_str) else { continue };
        let mut info = ModelInfo { id: id.to_string(), vision: None, tools: None, context: None };
        // OpenRouter puts the details in the list itself.
        if let Some(inputs) = m.pointer("/architecture/input_modalities").and_then(Value::as_array) {
            info.vision = Some(inputs.iter().any(|v| v == "image"));
        }
        if let Some(params) = m.get("supported_parameters").and_then(Value::as_array) {
            info.tools = Some(params.iter().any(|v| v == "tools"));
        }
        info.context = m.get("context_length").and_then(as_u32);
        out.push(info);
    }
    let root = root_of(base_url);
    match provider {
        "ollama" => {
            out.retain_mut(|info| {
                let Some(show) = post_json(&agent, &format!("{root}/api/show"), &json!({"model": info.id})) else { return true };
                let caps: Vec<&str> =
                    show.get("capabilities").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                if !caps.is_empty() && !caps.contains(&"completion") {
                    return false; // an embedding model
                }
                info.vision = Some(caps.contains(&"vision"));
                info.tools = Some(caps.contains(&"tools"));
                info.context = show
                    .get("model_info")
                    .and_then(Value::as_object)
                    .and_then(|o| o.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| as_u32(v)));
                true
            });
        }
        "lmstudio" => {
            if let Ok(v) = get_json(&agent, &format!("{root}/api/v0/models"), None) {
                for m in v.get("data").and_then(Value::as_array).into_iter().flatten() {
                    let id = m.get("id").and_then(Value::as_str).unwrap_or_default();
                    let kind = m.get("type").and_then(Value::as_str).unwrap_or_default();
                    if kind == "embeddings" {
                        out.retain(|i| i.id != id);
                        continue;
                    }
                    if let Some(info) = out.iter_mut().find(|i| i.id == id) {
                        info.vision = Some(kind == "vlm");
                        info.context = m.get("loaded_context_length").or_else(|| m.get("max_context_length")).and_then(as_u32);
                    }
                }
            }
        }
        "llamacpp" => {
            if let Ok(v) = get_json(&agent, &format!("{root}/props"), None) {
                let ctx = v.pointer("/default_generation_settings/n_ctx").and_then(as_u32);
                let vision = v.pointer("/modalities/vision").and_then(Value::as_bool);
                for info in &mut out {
                    info.context = ctx.or(info.context);
                    info.vision = vision.or(info.vision);
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

/// The local servers running now, with their models. Loopback only, a
/// short timeout each.
pub fn find_local() -> Vec<FoundServer> {
    PROVIDERS
        .iter()
        .filter(|p| p.local)
        .filter_map(|p| {
            let models = list_models(p.id, p.base_url, None).ok()?;
            Some(FoundServer { provider: p.id.into(), name: p.name.into(), base_url: p.base_url.into(), models })
        })
        .collect()
}

/// The context the loop plans for: the setting, else what the server says
/// (for Ollama, never more than its default run length), else a default.
pub fn context_for(provider: &str, setting: Option<u32>, info: Option<&ModelInfo>) -> u32 {
    if let Some(c) = setting {
        return c;
    }
    let told = info.and_then(|i| i.context);
    match provider {
        "ollama" => told.map_or(OLLAMA_DEFAULT_CONTEXT, |c| c.min(OLLAMA_DEFAULT_CONTEXT)),
        _ => told.unwrap_or(DEFAULT_CONTEXT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_comes_from_the_setting_then_the_server() {
        let i = ModelInfo { id: "m".into(), vision: None, tools: None, context: Some(40960) };
        assert_eq!(context_for("lmstudio", Some(16000), Some(&i)), 16000);
        assert_eq!(context_for("lmstudio", None, Some(&i)), 40960);
        assert_eq!(context_for("ollama", None, Some(&i)), OLLAMA_DEFAULT_CONTEXT);
        assert_eq!(context_for("openai", None, None), DEFAULT_CONTEXT);
        assert_eq!(root_of("http://127.0.0.1:11434/v1/"), "http://127.0.0.1:11434");
    }
}
