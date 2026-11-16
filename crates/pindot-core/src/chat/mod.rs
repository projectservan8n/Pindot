//! The chat window's logic, free of any UI: attachments (copied into the
//! data dir and read by the brain under a scoped rule), the session
//! transcript turned into chat items, and the list of past conversations.

pub mod attachments;
pub mod history;
pub mod transcript;

/// Typed turns that are not read aloud start with this note, so the brain
/// answers at normal length with Markdown (its system prompt otherwise says
/// "one or two short sentences, read aloud"). The chat view hides it.
pub const TYPED_NOTE: &str = "(Typed in the chat window and not read aloud: answer normally, Markdown is fine.) ";
