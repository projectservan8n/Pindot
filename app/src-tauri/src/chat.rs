//! The chat window's side of the app: it follows the assistant's session,
//! reads that session's transcript (read only) into chat items, and sends
//! typed messages into the same persistent session. Chat runs with voice
//! off. The logic lives in `pindot_core::chat`; this file is the glue to Tauri.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pindot_core::brain::transcript::find_transcript;
use pindot_core::chat::attachments::{self, Attached, Imported, Source};
use pindot_core::chat::history::{Entry, Index};
use pindot_core::chat::transcript::{ChatItem, Reader, read_all};
use pindot_core::voice::machine::{Display, Input};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::ui::{MAIN, Ui};

const POLL: Duration = Duration::from_millis(500);
const FIND_EVERY: Duration = Duration::from_secs(2);

pub struct ChatHub {
    inner: Mutex<Inner>,
}

struct Inner {
    session: Option<String>,
    display: Display,
    reader: Option<Reader>,
    last_find: Instant,
    index: Index,
    data_dir: Option<PathBuf>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl ChatHub {
    fn new() -> Self {
        let data_dir = pindot_core::paths::data_dir().ok();
        let index = data_dir.as_deref().map(Index::load).unwrap_or_default();
        if let Some(d) = &data_dir {
            let n = attachments::prune(d, SystemTime::now());
            if n > 0 {
                println!("chat: removed attachments of {n} old conversation(s)");
            }
        }
        ChatHub {
            inner: Mutex::new(Inner { session: None, display: Display::Idle, reader: None, last_find: Instant::now() - FIND_EVERY, index, data_dir }),
        }
    }

    /// The brain started (or restarted) a session: the chat follows it.
    pub fn session_ready(&self, id: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.session.as_deref() == Some(id) {
            return;
        }
        g.session = Some(id.to_string());
        g.reader = None;
        g.last_find = Instant::now() - FIND_EVERY;
        g.index.touch(id, now_secs(), None, None, None);
        save(&g);
    }

    pub fn set_display(&self, d: Display) {
        self.inner.lock().unwrap().display = d;
    }

    /// One poll of the transcript. Returns the session id when items changed.
    fn tick(&self) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        let sid = g.session.clone()?;
        if g.reader.is_none() {
            if g.last_find.elapsed() < FIND_EVERY {
                return None;
            }
            g.last_find = Instant::now();
            let p = find_transcript(&sid)?;
            g.index.touch(&sid, now_secs(), None, Some(&p), None);
            g.reader = Some(Reader::new(p));
        }
        let changed = g.reader.as_mut()?.poll();
        if !changed {
            return None;
        }
        let first = g.reader.as_ref().and_then(|r| {
            r.items().iter().find_map(|i| match i {
                ChatItem::User { text, .. } => Some(text.clone()),
                _ => None,
            })
        });
        g.index.touch(&sid, now_secs(), first.as_deref(), None, None);
        save(&g);
        Some(sid)
    }
}

fn save(g: &Inner) {
    if let Some(d) = &g.data_dir
        && let Err(e) = g.index.save(d)
    {
        tracing::warn!("chat history index not saved: {e:#}");
    }
}

/// Create the hub and its poller. Call once, after `Ui` is managed.
pub fn start(app: &AppHandle) {
    let hub = Arc::new(ChatHub::new());
    app.manage(hub.clone());
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("pindot-chat".into()).spawn(move || {
        loop {
            std::thread::sleep(POLL);
            if let Some(sid) = hub.tick() {
                let _ = app.emit("pindot:chat", serde_json::json!({ "session_id": sid }));
            }
        }
    });
    if let Err(e) = spawned {
        println!("chat: no transcript poller: {e}");
    }
}

fn main_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == MAIN { Ok(()) } else { Err("The chat can only be used from the Pindot window.".into()) }
}

#[derive(Serialize)]
pub struct ChatView {
    session_id: Option<String>,
    items: Vec<ChatItem>,
    display: Display,
    /// False when the assistant is not running (no typed messages then).
    can_send: bool,
    /// Replies can be read aloud: hold-to-talk and speech are running.
    can_speak: bool,
}

#[tauri::command]
pub fn chat_state(window: tauri::WebviewWindow, app: AppHandle, hub: tauri::State<'_, Arc<ChatHub>>) -> Result<ChatView, String> {
    main_only(&window)?;
    let can_send = app.state::<Ui>().voice_handle().is_some();
    let can_speak = app.state::<Ui>().speech_on();
    let g = hub.inner.lock().unwrap();
    Ok(ChatView {
        session_id: g.session.clone(),
        items: g.reader.as_ref().map(|r| r.items().to_vec()).unwrap_or_default(),
        display: g.display,
        can_send,
        can_speak,
    })
}

fn session_dir(g: &Inner) -> Result<(String, PathBuf), String> {
    let sid = g.session.clone().ok_or("The assistant is still starting. Try again in a moment.")?;
    let data = g.data_dir.clone().ok_or("Pindot has no data folder to keep attachments in.")?;
    let dir = attachments::session_dir_in(&data, &sid).map_err(|e| e.to_string())?;
    Ok((sid, dir))
}

fn import(hub: &ChatHub, sources: Vec<Source>) -> Result<Imported, String> {
    let dir = session_dir(&hub.inner.lock().unwrap())?.1;
    attachments::import(&dir, sources).map_err(|e| format!("Pindot could not save the files: {e}"))
}

/// Files the user dropped or picked (paths).
#[tauri::command]
pub fn chat_attach_paths(window: tauri::WebviewWindow, hub: tauri::State<'_, Arc<ChatHub>>, paths: Vec<String>) -> Result<Imported, String> {
    main_only(&window)?;
    import(&hub, paths.into_iter().map(|p| Source::Path(p.into())).collect())
}

/// A file pasted from the clipboard (bytes, base64).
#[tauri::command]
pub fn chat_attach_bytes(window: tauri::WebviewWindow, hub: tauri::State<'_, Arc<ChatHub>>, name: String, data: String) -> Result<Imported, String> {
    use base64::Engine;
    main_only(&window)?;
    // Refuse early: a base64 string of this length is far over any cap.
    if data.len() as u64 > attachments::MAX_FILE_BYTES * 4 / 3 + 16 {
        return Ok(Imported {
            attached: vec![],
            rejected: vec![attachments::Rejected { name, reason: "This is over the 10.0 MB limit. Attach a smaller one.".into() }],
        });
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).map_err(|_| "The pasted file could not be read.".to_string())?;
    import(&hub, vec![Source::Bytes { name, data: bytes }])
}

/// The "Attach" button: the system file picker, then the same import.
#[tauri::command]
pub async fn chat_choose_files(window: tauri::WebviewWindow, app: AppHandle) -> Result<Option<Imported>, String> {
    use tauri_plugin_dialog::DialogExt;
    main_only(&window)?;
    let Some(picked) = app.dialog().file().set_title("Attach files").blocking_pick_files() else { return Ok(None) };
    let paths: Vec<Source> = picked.into_iter().filter_map(|p| p.into_path().ok()).map(Source::Path).collect();
    let hub = app.state::<Arc<ChatHub>>();
    import(&hub, paths).map(Some)
}

/// Send a typed message (and attached files) into the voice session.
#[tauri::command]
pub fn chat_send(
    window: tauri::WebviewWindow,
    app: AppHandle,
    hub: tauri::State<'_, Arc<ChatHub>>,
    text: String,
    files: Vec<String>,
    speak: bool,
) -> Result<(), String> {
    main_only(&window)?;
    let Some(handle) = app.state::<Ui>().voice_handle() else {
        return Err("The assistant is not running. Finish Setup and sign in to Claude Code, then chat works.".into());
    };
    let dir = session_dir(&hub.inner.lock().unwrap())?.1;
    // Only files copied for this very conversation: Read is allowed in that folder alone.
    let dir_real = dir.canonicalize().ok();
    let mut attached = Vec::new();
    for f in files {
        let p = PathBuf::from(&f);
        let ok = match (p.canonicalize(), &dir_real) {
            (Ok(c), Some(d)) => c.starts_with(d),
            _ => false,
        };
        if !ok {
            return Err("An attached file belongs to an earlier conversation. Remove it and attach it again.".into());
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        attached.push(Attached { kind: attachments::kind_of(&name), name, path: p, size });
    }
    // Nothing can be read aloud while the voice parts are off.
    let speak = speak && app.state::<Ui>().speech_on();
    if text.trim().is_empty() && attached.is_empty() {
        return Err("Type a message first.".into());
    }
    let mut prompt = attachments::prompt_with(&text, &attached);
    if !speak {
        prompt = format!("{}{prompt}", pindot_core::chat::TYPED_NOTE);
    }
    {
        let mut g = hub.inner.lock().unwrap();
        if let Some(sid) = g.session.clone() {
            g.index.touch(&sid, now_secs(), Some(&prompt), None, None);
            save(&g);
        }
    }
    handle.input(Input::TypedPrompt { text: prompt, speak });
    Ok(())
}

/// The Stop button: the same as pressing Esc.
#[tauri::command]
pub fn chat_stop(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    main_only(&window)?;
    if let Some(state) = app.try_state::<Arc<pindot_core::server::ServerState>>() {
        state.control.emergency_stop();
    } else if let Some(h) = app.state::<Ui>().voice_handle() {
        h.input(Input::Esc);
    }
    Ok(())
}

/// An attached image as a data URL, so the page can show it under its strict
/// policy. Only files inside Pindot's own attachments folder, only images,
/// and only up to the image cap.
#[tauri::command]
pub fn chat_image(window: tauri::WebviewWindow, hub: tauri::State<'_, Arc<ChatHub>>, path: String) -> Result<String, String> {
    use base64::Engine;
    main_only(&window)?;
    let data = hub.inner.lock().unwrap().data_dir.clone().ok_or("No data folder.")?;
    let root = attachments::root_in(&data).canonicalize().map_err(|_| "That picture is no longer on this computer.")?;
    let p = PathBuf::from(&path).canonicalize().map_err(|_| "That picture is no longer on this computer.")?;
    if !p.starts_with(&root) {
        return Err("Only pictures attached in this chat can be shown.".into());
    }
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if attachments::kind_of(&name) != attachments::Kind::Image {
        return Err("That file is not a picture.".into());
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if size > attachments::MAX_IMAGE_BYTES {
        return Err("That picture is too big to show here.".into());
    }
    let mime = match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    let bytes = std::fs::read(&p).map_err(|e| format!("Could not read the picture: {e}"))?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
pub fn chat_history(window: tauri::WebviewWindow, hub: tauri::State<'_, Arc<ChatHub>>) -> Result<Vec<Entry>, String> {
    main_only(&window)?;
    Ok(hub.inner.lock().unwrap().index.list())
}

#[derive(Serialize)]
pub struct Opened {
    title: String,
    items: Vec<ChatItem>,
}

/// A past conversation, read only: its transcript, found by the id in
/// Pindot's own index (or by scanning for that id), never by a path the page
/// supplies.
#[tauri::command]
pub fn chat_open(window: tauri::WebviewWindow, hub: tauri::State<'_, Arc<ChatHub>>, session_id: String) -> Result<Opened, String> {
    main_only(&window)?;
    if uuid::Uuid::parse_str(&session_id).is_err() {
        return Err("That is not a conversation id.".into());
    }
    let (title, known) = {
        let g = hub.inner.lock().unwrap();
        match g.index.find(&session_id) {
            Some(e) => (e.title.clone(), e.transcript.clone()),
            None => (String::new(), None),
        }
    };
    let path = known.filter(|p| p.is_file()).or_else(|| find_transcript(&session_id)).ok_or("This conversation's record is no longer on this computer.")?;
    Ok(Opened { title, items: read_all(&path) })
}
