//! Pindot's own loop: a brain for local models and OpenAI-compatible APIs.
//! A worker thread owns one conversation at a time and reports to the
//! voice machine with the same `Input`s as the Claude Code brain. The
//! desktop tools are reached through Pindot's own MCP server, so
//! approvals, Esc, the overlay and the activity log apply unchanged.

pub mod anthropic;
pub mod context;
pub mod log;
pub mod mcp_client;
pub mod openai;
pub mod probe;
pub mod providers;
pub mod run;
pub mod secrets;
pub mod settings;
pub mod test_model;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;

use self::log::ConversationLog;
use self::mcp_client::McpClient;
use self::openai::{CallError, Endpoint, Image, Msg, Reply, ToolDef, Who, chat, describe, request_body};
use self::providers::Wire;
use self::run::{Conversation, LIMITS, Model, TurnEnd, TurnEvent, run_turn};
use self::settings::{BrainSettings, Vision};
use super::Brain;
use super::persistent::InputSink;
use crate::chat::attachments::{Kind, kind_of, split_prompt};
use crate::server::ServerState;
use crate::voice::machine::Input;

const RETRY_FIRST: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(300);
/// No first token (a local model loading counts) or no next chunk for this long.
pub const IDLE: Duration = Duration::from_secs(120);
/// Attached text files go in whole up to this size.
const TEXT_FILE_MAX: u64 = 32 * 1024;
/// Left out for a model that cannot see.
const PICTURE_TOOLS: &[&str] = &["screenshot", "zoom"];

pub struct LoopConfig {
    pub brain: BrainSettings,
    pub key: Option<String>,
    pub mcp_url: String,
    pub token: String,
    pub system_prompt: String,
    /// Conversations are written under it (`conversations/`).
    pub data_dir: PathBuf,
}

/// The HTTP model in use.
pub struct HttpModel {
    pub endpoint: Endpoint,
    pub model: String,
}

impl HttpModel {
    /// The model a brain setting names, in the format its provider speaks.
    pub fn new(brain: &BrainSettings, key: Option<String>) -> Self {
        let wire = providers::find(&brain.provider).map_or(Wire::OpenAi, |p| p.wire);
        Self { endpoint: Endpoint { base_url: brain.base_url.clone(), key, wire }, model: brain.model.clone() }
    }
}

impl Model for HttpModel {
    fn reply(&mut self, messages: &[Msg], tools: &[ToolDef], cancel: &AtomicBool) -> Result<Reply, CallError> {
        match self.endpoint.wire {
            Wire::OpenAi => chat(&self.endpoint, &request_body(&self.model, messages, tools), cancel, IDLE),
            Wire::Anthropic => anthropic::chat(&self.endpoint, &anthropic::request_body(&self.model, messages, tools), cancel, IDLE),
        }
    }
}

/// Who answers, for messages.
pub fn who(brain: &BrainSettings) -> Who {
    let provider = providers::find(&brain.provider);
    Who {
        provider: provider.map_or_else(|| brain.provider.clone(), |p| p.name.to_string()),
        model: brain.model.clone(),
        local: provider.is_some_and(|p| p.local),
    }
}

/// The system prompt with the line about the tools this model has.
pub fn system_prompt(base: &str, vision: bool) -> String {
    let tools = if vision {
        "Use the desktop tools by name with JSON arguments. Prefer read_window and find; take a screenshot only when text is not enough."
    } else {
        "Use the desktop tools by name with JSON arguments. You cannot see pictures, so you have no screenshot tool: use read_window and find."
    };
    format!("{base} {tools}").trim().to_string()
}

/// The tools a model gets: no picture tools for a model that cannot see.
pub fn tools_for(all: Vec<ToolDef>, vision: bool) -> Vec<ToolDef> {
    all.into_iter().filter(|t| vision || !PICTURE_TOOLS.contains(&t.name.as_str())).collect()
}

/// The user's message: attached pictures as images (or a note), small text
/// files inline, anything else a note.
pub fn user_message(prompt: &str, vision: bool) -> Msg {
    let (mut text, paths) = split_prompt(prompt);
    let mut images = vec![];
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let cannot = format!("\n\n(I can't open the attached file {name} with this brain.)");
        match kind_of(&name) {
            Kind::Image if vision => match std::fs::read(&p) {
                Ok(bytes) => {
                    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                    let mime = if ext == "jpg" { "image/jpeg".to_string() } else { format!("image/{ext}") };
                    images.push(Image { mime, base64: base64::engine::general_purpose::STANDARD.encode(bytes) });
                }
                Err(_) => text.push_str(&cannot),
            },
            Kind::File if std::fs::metadata(&p).is_ok_and(|m| m.len() <= TEXT_FILE_MAX) => match std::fs::read_to_string(&p) {
                Ok(body) => text.push_str(&format!("\n\nAttached file {name}:\n```\n{body}\n```")),
                Err(_) => text.push_str(&cannot),
            },
            _ => text.push_str(&cannot),
        }
    }
    Msg::User { text, images }
}

enum Command {
    Send(String),
    NewSession,
    Shutdown,
}

/// Handle to the loop's worker. Dropping it ends the conversation.
pub struct LoopBrain {
    tx: mpsc::Sender<Command>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopBrain {
    pub fn start(config: LoopConfig, state: Arc<ServerState>, emit: InputSink) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let thread =
            std::thread::Builder::new().name("pindot-loop-brain".into()).spawn(move || Worker::new(config, state, emit, c).run(rx))?;
        Ok(Self { tx, cancel, thread: Some(thread) })
    }
}

impl Brain for LoopBrain {
    fn send(&self, text: &str) {
        let _ = self.tx.send(Command::Send(text.to_string()));
    }
    fn interrupt(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    fn new_session(&self) {
        let _ = self.tx.send(Command::NewSession);
    }
}

impl Drop for LoopBrain {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Session {
    id: String,
    mcp: McpClient,
    tools: Vec<ToolDef>,
    conv: Conversation,
    log: Option<ConversationLog>,
}

struct Worker {
    config: LoopConfig,
    state: Arc<ServerState>,
    emit: InputSink,
    cancel: Arc<AtomicBool>,
    model: HttpModel,
    who: Who,
    session: Option<Session>,
    /// Why no session could be opened, said once until it changes.
    problem: Option<String>,
}

impl Worker {
    fn new(config: LoopConfig, state: Arc<ServerState>, emit: InputSink, cancel: Arc<AtomicBool>) -> Self {
        let who = who(&config.brain);
        let model = HttpModel::new(&config.brain, config.key.clone());
        Self { config, state, emit, cancel, model, who, session: None, problem: None }
    }

    fn run(mut self, rx: mpsc::Receiver<Command>) {
        let mut retry = RETRY_FIRST;
        let mut next_try = Instant::now();
        loop {
            if self.session.is_none() && Instant::now() >= next_try {
                match self.open_session() {
                    Ok(()) => retry = RETRY_FIRST,
                    Err(msg) => {
                        if self.problem.as_deref() != Some(msg.as_str()) {
                            println!("loop brain: {msg}");
                            (self.emit)(Input::BrainUnavailable { message: msg.clone() });
                        }
                        self.problem = Some(msg);
                        next_try = Instant::now() + retry;
                        retry = (retry * 2).min(RETRY_MAX);
                    }
                }
            }
            let wait = if self.session.is_none() {
                next_try.saturating_duration_since(Instant::now()).max(Duration::from_millis(100))
            } else {
                Duration::from_secs(3600)
            };
            match rx.recv_timeout(wait) {
                Ok(Command::Send(text)) => self.turn(&text),
                Ok(Command::NewSession) => {
                    self.close_session();
                    next_try = Instant::now();
                    retry = RETRY_FIRST;
                }
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.close_session();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn open_session(&mut self) -> Result<(), String> {
        let b = &self.config.brain;
        let models = probe::list_models(&b.provider, &b.base_url, self.config.key.as_deref()).map_err(|e| describe(&e, &self.who))?;
        let info = models.iter().find(|m| m.id == b.model);
        if info.is_none() && !models.is_empty() {
            return Err(describe(&CallError::Status { code: 404, body: String::new() }, &self.who));
        }
        let vision = match b.vision {
            Vision::On => true,
            Vision::Off => false,
            Vision::Auto => info.and_then(|i| i.vision).unwrap_or(false),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mcp = McpClient::new(&self.config.mcp_url, &self.config.token, &id);
        let tools = tools_for(mcp.list_tools()?, vision);
        let context = probe::context_for(&b.provider, b.context_tokens, info) as usize;
        let log = ConversationLog::create(&crate::brain::transcript::conversations_dir(&self.config.data_dir), &id)
            .map_err(|e| tracing::warn!("conversation file not created: {e:#}"))
            .ok();
        let conv =
            Conversation { messages: vec![Msg::System(system_prompt(&self.config.system_prompt, vision))], context_tokens: context, vision };
        println!("loop brain: session {id} ready ({} on {}, vision {vision}, context {context}, {} tools)", b.model, self.who.provider, tools.len());
        self.session = Some(Session { id: id.clone(), mcp, tools, conv, log });
        self.problem = None;
        (self.emit)(Input::SessionReady { session_id: id });
        Ok(())
    }

    fn close_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.mcp.close();
            self.state.end_session(&s.id);
        }
    }

    fn turn(&mut self, text: &str) {
        let Some(s) = self.session.as_mut() else {
            let message = self.problem.clone().unwrap_or_else(|| "The assistant is still starting. Try again in a moment.".into());
            (self.emit)(Input::TurnFailed { message });
            return;
        };
        self.cancel.store(false, Ordering::SeqCst);
        let sid = s.id.clone();
        s.conv.messages.push(user_message(text, s.conv.vision));
        if let Some(l) = &s.log {
            l.user(text);
        }
        (self.emit)(Input::TranscriptPrompt { session_id: sid.clone() });
        let emit = self.emit.clone();
        let end = run_turn(&mut s.conv, &mut self.model, &s.tools, &mut s.mcp, s.log.as_ref(), &self.cancel, &LIMITS, &self.who, &mut |e| {
            emit(match e {
                TurnEvent::Text(text) => Input::TranscriptText { session_id: sid.clone(), text },
                TurnEvent::ToolUse(name) => Input::TranscriptToolUse { session_id: sid.clone(), name: format!("{}{name}", log::TOOL_PREFIX) },
                TurnEvent::ToolResult => Input::TranscriptToolResult { session_id: sid.clone() },
            })
        });
        match end {
            TurnEnd::Done { last } => (self.emit)(Input::Stop { session_id: sid, last_message: last }),
            TurnEnd::StepLimit => (self.emit)(Input::Stop {
                session_id: sid,
                last_message: Some(format!("I've stopped after {} steps.", LIMITS.max_tool_calls)),
            }),
            // The voice machine already ended the turn when it sent Esc.
            TurnEnd::Interrupted => {}
            TurnEnd::Failed(message) => (self.emit)(Input::TurnFailed { message }),
            TurnEnd::TooLong(message) => {
                (self.emit)(Input::TurnFailed { message });
                self.close_session();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> ToolDef {
        ToolDef { name: name.into(), description: String::new(), parameters: json!({}) }
    }

    #[test]
    fn a_model_that_cannot_see_gets_no_picture_tools() {
        let all = || vec![tool("click"), tool("screenshot"), tool("zoom"), tool("find")];
        assert_eq!(tools_for(all(), true).len(), 4);
        let names: Vec<String> = tools_for(all(), false).into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["click", "find"]);
        assert!(system_prompt("Be brief.", false).contains("no screenshot tool"));
    }

    #[test]
    fn attachments_become_pictures_text_or_a_note() {
        let dir = std::env::temp_dir().join(format!("pindot-attach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (pic, txt, pdf) = (dir.join("a.png"), dir.join("notes.txt"), dir.join("doc.pdf"));
        std::fs::write(&pic, [1u8, 2, 3]).unwrap();
        std::fs::write(&txt, "milk, eggs").unwrap();
        std::fs::write(&pdf, "%PDF").unwrap();
        let prompt = crate::chat::attachments::prompt_with(
            "look",
            &[pic.clone(), txt.clone(), pdf.clone()]
                .iter()
                .map(|p| crate::chat::attachments::Attached {
                    kind: kind_of(&p.file_name().unwrap().to_string_lossy()),
                    name: p.file_name().unwrap().to_string_lossy().into_owned(),
                    path: p.clone(),
                    size: 3,
                })
                .collect::<Vec<_>>(),
        );
        let Msg::User { text, images } = user_message(&prompt, true) else { panic!() };
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
        assert!(text.starts_with("look"), "{text}");
        assert!(text.contains("milk, eggs") && text.contains("can't open the attached file doc.pdf"), "{text}");
        let Msg::User { text, images } = user_message(&prompt, false) else { panic!() };
        assert!(images.is_empty() && text.contains("can't open the attached file a.png"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
