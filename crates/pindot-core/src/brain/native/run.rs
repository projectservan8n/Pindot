//! One turn of Pindot's own loop: ask the model, run the tool calls it
//! makes, give it the results, and repeat until it answers without tools.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::context;
use super::log::ConversationLog;
use super::mcp_client::{McpClient, ToolOutput};
use super::openai::{CallError, Image, Msg, Reply, ToolCall, ToolDef, Who, describe, is_context_overflow};

/// What answers a request: the HTTP model in use, or a script in tests.
pub trait Model: Send {
    fn reply(&mut self, messages: &[Msg], tools: &[ToolDef], cancel: &AtomicBool) -> Result<Reply, CallError>;
}

/// What runs a tool call: the MCP client, or a fake in tests.
pub trait Tools: Send {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput;
}

impl Tools for McpClient {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput {
        McpClient::call(self, name, arguments)
    }
}

impl Tools for &McpClient {
    fn call(&mut self, name: &str, arguments: &str) -> ToolOutput {
        McpClient::call(self, name, arguments)
    }
}

pub struct Conversation {
    /// Starts with the system prompt.
    pub messages: Vec<Msg>,
    pub context_tokens: usize,
    /// The model can see pictures.
    pub vision: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// A complete assistant message with text.
    Text(String),
    /// A tool call is starting (the tool's own name).
    ToolUse(String),
    ToolResult,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    Done { last: Option<String> },
    /// `max_tool_calls` reached.
    StepLimit,
    Interrupted,
    Failed(String),
    /// Too long even after trimming: start a fresh conversation.
    TooLong(String),
}

#[derive(Debug, Clone, Copy)]
pub struct TurnLimits {
    pub max_tool_calls: usize,
    pub deadline: Duration,
}

pub const LIMITS: TurnLimits = TurnLimits { max_tool_calls: 25, deadline: crate::brain::persistent::TURN_TIMEOUT };

/// The share of the context the conversation may fill (the rest is for
/// the reply).
const FILL: f64 = 0.85;

#[allow(clippy::too_many_arguments)]
pub fn run_turn(
    conv: &mut Conversation,
    model: &mut dyn Model,
    tools: &[ToolDef],
    runner: &mut dyn Tools,
    log: Option<&ConversationLog>,
    cancel: &AtomicBool,
    limits: &TurnLimits,
    who: &Who,
    emit: &mut dyn FnMut(TurnEvent),
) -> TurnEnd {
    let started = Instant::now();
    let mut calls_made = 0;
    let mut last_text: Option<String> = None;
    // The tool list goes with every request and takes its share too.
    let tools_tokens: usize = tools.iter().map(|t| (t.name.len() + t.description.len() + t.parameters.to_string().len()) / 4).sum();
    let budget = ((conv.context_tokens as f64 * FILL) as usize).saturating_sub(tools_tokens);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return interrupted(log);
        }
        if started.elapsed() > limits.deadline {
            return TurnEnd::Failed("This took longer than 15 minutes, so I stopped.".into());
        }
        if context::fit(&mut conv.messages, budget) {
            context::strip_thinking(&mut conv.messages);
        }
        let reply = match model.reply(&conv.messages, tools, cancel) {
            Ok(r) => r,
            Err(CallError::Cancelled) => return interrupted(log),
            Err(e) if is_context_overflow(&e) => {
                if context::fit(&mut conv.messages, budget / 2) {
                    context::strip_thinking(&mut conv.messages);
                }
                match model.reply(&conv.messages, tools, cancel) {
                    Ok(r) => r,
                    Err(CallError::Cancelled) => return interrupted(log),
                    Err(e) if is_context_overflow(&e) => {
                        return TurnEnd::TooLong(format!("This conversation got too long for {}. I'll start a fresh one.", who.model));
                    }
                    Err(e) => return TurnEnd::Failed(describe(&e, who)),
                }
            }
            Err(e) => return TurnEnd::Failed(describe(&e, who)),
        };
        if let Some(l) = log {
            l.assistant(&reply.thinking, &reply.text, &reply.calls);
        }
        conv.messages.push(Msg::Assistant { text: reply.text.clone(), calls: reply.calls.clone(), raw: reply.raw.clone() });
        if !reply.text.is_empty() {
            last_text = Some(reply.text.clone());
            emit(TurnEvent::Text(reply.text.clone()));
        }
        if reply.calls.is_empty() {
            return TurnEnd::Done { last: last_text };
        }
        let mut pictures: Vec<Image> = vec![];
        let mut stopped = false;
        for c in &reply.calls {
            if stopped || cancel.load(Ordering::SeqCst) {
                stopped = true;
                answer(conv, log, c, "Not run: the user stopped the turn.", true);
                continue;
            }
            emit(TurnEvent::ToolUse(c.name.clone()));
            let out = runner.call(&c.name, &c.arguments);
            answer(conv, log, c, &out.text, out.is_error);
            emit(TurnEvent::ToolResult);
            if conv.vision {
                pictures.extend(out.images);
            }
            calls_made += 1;
        }
        if !pictures.is_empty() {
            conv.messages.push(Msg::ToolImages(pictures));
        }
        if stopped || cancel.load(Ordering::SeqCst) {
            return interrupted(log);
        }
        if calls_made >= limits.max_tool_calls {
            return TurnEnd::StepLimit;
        }
    }
}

/// Every tool call gets a result, even a skipped one, or the next request
/// would be refused for a call without an answer.
fn answer(conv: &mut Conversation, log: Option<&ConversationLog>, c: &ToolCall, text: &str, is_error: bool) {
    let text = if text.is_empty() { "(no output)" } else { text };
    if let Some(l) = log {
        l.tool_result(&c.id, text, is_error);
    }
    conv.messages.push(Msg::Tool { call_id: c.id.clone(), text: text.to_string() });
}

fn interrupted(log: Option<&ConversationLog>) -> TurnEnd {
    if let Some(l) = log {
        l.interrupted();
    }
    TurnEnd::Interrupted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    struct Script(VecDeque<Result<Reply, CallError>>);
    impl Model for Script {
        fn reply(&mut self, _m: &[Msg], _t: &[ToolDef], _c: &AtomicBool) -> Result<Reply, CallError> {
            self.0.pop_front().unwrap_or_else(|| Ok(Reply { text: "end".into(), ..Default::default() }))
        }
    }

    #[derive(Default)]
    struct Fake {
        called: Vec<String>,
        cancel_on: Option<(String, Arc<AtomicBool>)>,
    }
    impl Tools for Fake {
        fn call(&mut self, name: &str, _a: &str) -> ToolOutput {
            self.called.push(name.to_string());
            if let Some((n, c)) = &self.cancel_on
                && n == name
            {
                c.store(true, Ordering::SeqCst);
            }
            let images = if name == "screenshot" { vec![Image { mime: "image/png".into(), base64: "AA".into() }] } else { vec![] };
            ToolOutput { text: format!("{name} ok"), images, is_error: false }
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall { id: id.into(), name: name.into(), arguments: "{}".into() }
    }
    fn who() -> Who {
        Who { provider: "Ollama".into(), model: "m".into(), local: true }
    }
    fn conv() -> Conversation {
        Conversation {
            messages: vec![Msg::System("s".into()), Msg::User { text: "go".into(), images: vec![] }],
            context_tokens: 32_000,
            vision: true,
        }
    }
    const L: TurnLimits = TurnLimits { max_tool_calls: 25, deadline: Duration::from_secs(60) };

    fn run(c: &mut Conversation, model: &mut Script, tools: &mut Fake, cancel: &AtomicBool, limits: &TurnLimits) -> (TurnEnd, Vec<TurnEvent>) {
        let mut events = vec![];
        let end = run_turn(c, model, &[], tools, None, cancel, limits, &who(), &mut |e| events.push(e));
        (end, events)
    }

    #[test]
    fn tools_are_called_until_a_reply_without_calls() {
        let mut model = Script(VecDeque::from([
            Ok(Reply { text: "Looking.".into(), calls: vec![call("a", "screenshot"), call("b", "find")], ..Default::default() }),
            Ok(Reply { text: "Found it.".into(), ..Default::default() }),
        ]));
        let mut tools = Fake::default();
        let mut c = conv();
        let (end, events) = run(&mut c, &mut model, &mut tools, &AtomicBool::new(false), &L);
        assert_eq!(end, TurnEnd::Done { last: Some("Found it.".into()) });
        assert_eq!(tools.called, vec!["screenshot", "find"]);
        assert_eq!(
            events,
            vec![
                TurnEvent::Text("Looking.".into()),
                TurnEvent::ToolUse("screenshot".into()),
                TurnEvent::ToolResult,
                TurnEvent::ToolUse("find".into()),
                TurnEvent::ToolResult,
                TurnEvent::Text("Found it.".into()),
            ]
        );
        // system, user, assistant, tool, tool, picture, assistant
        assert!(matches!(c.messages[5], Msg::ToolImages(_)));
        assert_eq!(c.messages.len(), 7);
    }

    #[test]
    fn no_pictures_go_to_a_model_that_cannot_see() {
        let mut model = Script(VecDeque::from([Ok(Reply { calls: vec![call("a", "screenshot")], ..Default::default() })]));
        let mut c = Conversation { vision: false, ..conv() };
        run(&mut c, &mut model, &mut Fake::default(), &AtomicBool::new(false), &L);
        assert!(!c.messages.iter().any(|m| matches!(m, Msg::ToolImages(_))));
    }

    #[test]
    fn esc_between_calls_skips_the_rest_and_answers_every_call() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut model = Script(VecDeque::from([Ok(Reply { calls: vec![call("a", "click"), call("b", "type_text")], ..Default::default() })]));
        let mut tools = Fake { cancel_on: Some(("click".into(), cancel.clone())), ..Default::default() };
        let mut c = conv();
        let (end, _) = run(&mut c, &mut model, &mut tools, &cancel, &L);
        assert_eq!(end, TurnEnd::Interrupted);
        assert_eq!(tools.called, vec!["click"]);
        assert!(matches!(&c.messages[4], Msg::Tool { call_id, text } if call_id == "b" && text.contains("stopped")), "{:?}", c.messages);
    }

    #[test]
    fn the_step_limit_ends_the_turn() {
        let replies = (0..10).map(|i| Ok(Reply { calls: vec![call(&format!("c{i}"), "wait")], ..Default::default() })).collect();
        let limits = TurnLimits { max_tool_calls: 3, ..L };
        let (end, _) = run(&mut conv(), &mut Script(replies), &mut Fake::default(), &AtomicBool::new(false), &limits);
        assert_eq!(end, TurnEnd::StepLimit);
    }

    #[test]
    fn a_context_overflow_is_trimmed_and_tried_again_once() {
        let overflow = || Err(CallError::Status { code: 400, body: "maximum context length exceeded".into() });
        let mut model = Script(VecDeque::from([overflow(), Ok(Reply { text: "ok".into(), ..Default::default() })]));
        let (end, _) = run(&mut conv(), &mut model, &mut Fake::default(), &AtomicBool::new(false), &L);
        assert_eq!(end, TurnEnd::Done { last: Some("ok".into()) });
        let mut model = Script(VecDeque::from([overflow(), overflow()]));
        let (end, _) = run(&mut conv(), &mut model, &mut Fake::default(), &AtomicBool::new(false), &L);
        assert!(matches!(end, TurnEnd::TooLong(_)), "{end:?}");
    }

    #[test]
    fn an_error_is_said_plainly() {
        let mut model = Script(VecDeque::from([Err(CallError::Unreachable("refused".into()))]));
        let (end, _) = run(&mut conv(), &mut model, &mut Fake::default(), &AtomicBool::new(false), &L);
        assert_eq!(end, TurnEnd::Failed("Ollama isn't running. Start it, or pick another brain in Settings.".into()));
    }

    #[test]
    fn the_tool_list_counts_against_the_context() {
        struct Count(usize);
        impl Model for Count {
            fn reply(&mut self, m: &[Msg], _t: &[ToolDef], _c: &AtomicBool) -> Result<Reply, CallError> {
                self.0 = m.len();
                Ok(Reply { text: "ok".into(), ..Default::default() })
            }
        }
        let big = ToolDef { name: "t".into(), description: "x".repeat(4000), parameters: serde_json::json!({}) };
        let mut c = Conversation { messages: vec![Msg::System("s".into())], context_tokens: 2000, vision: false };
        for n in 0..6 {
            c.messages.push(Msg::User { text: format!("old request {n} {}", "y".repeat(600)), images: vec![] });
            c.messages.push(Msg::Assistant { text: "done".into(), calls: vec![], raw: None });
        }
        c.messages.push(Msg::User { text: "now".into(), images: vec![] });
        let mut model = Count(0);
        // Without tools everything fits; with 1000 tokens of tools, old turns go.
        run_turn(&mut c, &mut model, &[], &mut Fake::default(), None, &AtomicBool::new(false), &L, &who(), &mut |_| {});
        assert_eq!(model.0, 14);
        let mut c2 = Conversation { messages: c.messages[..14].to_vec(), context_tokens: 2000, vision: false };
        run_turn(&mut c2, &mut model, &[big], &mut Fake::default(), None, &AtomicBool::new(false), &L, &who(), &mut |_| {});
        assert!(model.0 < 14, "{}", model.0);
    }
}
