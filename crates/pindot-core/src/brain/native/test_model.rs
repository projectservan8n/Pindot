//! The model test: two short turns when a model is chosen. 1: a plain
//! reply ("ready"); 2: one call to `desktop_context` (read-only, needs no
//! approval) and an answer. It runs the loop directly, in a session of its
//! own that is ended afterwards, and writes no conversation file.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::mcp_client::McpClient;
use super::openai::Msg;
use super::run::{Conversation, TurnEnd, TurnEvent, TurnLimits, run_turn};
use super::settings::BrainSettings;
use super::{HttpModel, system_prompt, who};
use crate::server::ServerState;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModelTest {
    pub ok: bool,
    /// One sentence on what failed; empty when it passed.
    pub note: String,
}

const LIMITS: TurnLimits = TurnLimits { max_tool_calls: 3, deadline: Duration::from_secs(180) };

pub fn test_model(brain: &BrainSettings, key: Option<String>, mcp_url: &str, token: &str, state: &ServerState) -> ModelTest {
    let who = who(brain);
    let mut model = HttpModel::new(brain, key);
    let session = format!("model-test-{}", uuid::Uuid::new_v4());
    let mcp = McpClient::new(mcp_url, token, &session);
    let result = (|| -> Result<(), String> {
        let tools = mcp.list_tools().map_err(|e| format!("Pindot's desktop tools did not answer: {e}"))?;
        let system = system_prompt("You are being tested. Do exactly what the user asks, briefly.", false);
        let cancel = AtomicBool::new(false);
        let conv = |text: &str| Conversation {
            messages: vec![Msg::System(system.clone()), Msg::User { text: text.into(), images: vec![] }],
            context_tokens: 32_768,
            vision: false,
        };
        let mut runner = &mcp;
        let mut first = conv("Reply with the single word: ready.");
        match run_turn(&mut first, &mut model, &[], &mut runner, None, &cancel, &LIMITS, &who, &mut |_| {}) {
            TurnEnd::Done { last: Some(t) } if t.to_ascii_lowercase().contains("ready") => {}
            TurnEnd::Done { last } => {
                let said: String = last.unwrap_or_default().chars().take(80).collect();
                return Err(format!("It didn't answer as asked (it said: \"{said}\")."));
            }
            TurnEnd::Failed(m) | TurnEnd::TooLong(m) => return Err(m),
            _ => return Err("It didn't answer.".into()),
        }
        let mut second = conv("Call the desktop_context tool, then say how many displays there are.");
        let mut used = false;
        let end = run_turn(&mut second, &mut model, &tools, &mut runner, None, &cancel, &LIMITS, &who, &mut |e| {
            if e == TurnEvent::ToolUse("desktop_context".into()) {
                used = true;
            }
        });
        match end {
            TurnEnd::Failed(m) | TurnEnd::TooLong(m) => Err(m),
            _ if !used => Err("It answered but didn't call the tool: this model may not support tools.".into()),
            TurnEnd::Done { .. } => Ok(()),
            _ => Err("It called the tool but didn't finish its answer.".into()),
        }
    })();
    mcp.close();
    state.end_session(&session);
    match result {
        Ok(()) => ModelTest { ok: true, note: String::new() },
        Err(note) => ModelTest { ok: false, note },
    }
}
