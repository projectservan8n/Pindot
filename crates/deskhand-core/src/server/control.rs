//! Who controls input, and how the user takes it back.
//!
//! - **Lock:** one MCP session (one `claude` process) controls input at a
//!   time. The first action takes the lock; another session's actions are
//!   refused with the holder's name until the holder's session ends or has
//!   been idle for `LOCK_STALE`.
//! - **Burst:** while actions run (and for `idle` after the last one) the
//!   global Esc tap is armed and the "Claude is controlling input"
//!   indicator is on. If the tap cannot start, no action runs at all.
//! - **Emergency stop:** a real Esc press cancels the action in flight,
//!   refuses further actions until the user's next prompt (or a fallback
//!   timeout), and broadcasts `ControlEvent::EmergencyStop` so the voice loop
//!   can stop speech and interrupt the Claude turn.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Notify, broadcast};

use super::activity::ActivityLog;
use crate::platform::{ListenerHandle, Overlay, StopKey};

/// A lock holder that has not acted for this long loses the lock.
pub const LOCK_STALE: Duration = Duration::from_secs(120);
/// After Esc, actions stay refused until the next user prompt, or this long.
pub const HALT_FALLBACK: Duration = Duration::from_secs(60);
/// The Esc tap and indicator stay on this long after the last action.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlEvent {
    /// The user pressed Esc: stop speaking and interrupt the Claude turn.
    EmergencyStop,
    /// The indicator turned on or off.
    Acting { on: bool },
}

struct LockState {
    holder: Option<String>,
    last_used: Instant,
}

struct Burst {
    handle: Option<ListenerHandle>,
    /// Bumped on every action start, so an older idle timer does nothing.
    generation: u64,
}

pub struct Control {
    stop_key: Arc<dyn StopKey>,
    overlay: Arc<dyn Overlay>,
    activity: Arc<ActivityLog>,
    idle: Duration,
    lock: Mutex<LockState>,
    halted: Mutex<Option<Instant>>,
    cancel_gen: AtomicU64,
    cancel_notify: Notify,
    burst: Mutex<Burst>,
    active: AtomicUsize,
    approval_pending: AtomicBool,
    events: broadcast::Sender<ControlEvent>,
    me: Weak<Control>,
}

impl Control {
    pub fn new(
        stop_key: Arc<dyn StopKey>,
        overlay: Arc<dyn Overlay>,
        activity: Arc<ActivityLog>,
        idle: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            stop_key,
            overlay,
            activity,
            idle,
            lock: Mutex::new(LockState { holder: None, last_used: Instant::now() }),
            halted: Mutex::new(None),
            cancel_gen: AtomicU64::new(0),
            cancel_notify: Notify::new(),
            burst: Mutex::new(Burst { handle: None, generation: 0 }),
            active: AtomicUsize::new(0),
            approval_pending: AtomicBool::new(false),
            events: broadcast::channel(32).0,
            me: me.clone(),
        })
    }

    /// The hook for the voice loop and the brain: `EmergencyStop` means stop
    /// speech now and interrupt the running Claude turn.
    pub fn subscribe(&self) -> broadcast::Receiver<ControlEvent> {
        self.events.subscribe()
    }

    /// Start one input action for `session`. Refuses if the user pressed Esc,
    /// an approval prompt is open, another session holds the lock, or the Esc
    /// tap cannot be armed.
    pub fn begin_action(&self, session: &str) -> Result<ActionGuard, String> {
        {
            let mut h = self.halted.lock().unwrap();
            if let Some(at) = *h {
                if at.elapsed() < HALT_FALLBACK {
                    return Err("The user pressed Esc to stop you. Do not act again until the user sends a new message.".into());
                }
                *h = None;
            }
        }
        if self.approval_pending.load(Ordering::SeqCst) {
            return Err("An approval prompt is open. Wait for the user's answer before acting.".into());
        }
        {
            let mut l = self.lock.lock().unwrap();
            match &l.holder {
                Some(h) if h != session && l.last_used.elapsed() < LOCK_STALE => {
                    return Err(format!(
                        "Another Claude session ({h}) is controlling this computer. Only one session may control input at a time; try again when it has finished."
                    ));
                }
                Some(h) if h != session => {
                    tracing::info!(old = %h, new = %session, "lock was stale; handing it over");
                    self.activity.record(session, "lock", "", &format!("took over the stale lock from {h}"), true);
                    l.holder = Some(session.to_string());
                }
                Some(_) => {}
                None => {
                    l.holder = Some(session.to_string());
                    self.activity.record(session, "lock", "", "took the input lock", true);
                }
            }
            l.last_used = Instant::now();
        }
        {
            let mut b = self.burst.lock().unwrap();
            if b.handle.is_none() {
                let me = self.me.clone();
                let on_press: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    if let Some(c) = me.upgrade() {
                        c.emergency_stop();
                    }
                });
                match self.stop_key.start(on_press) {
                    Ok(h) => {
                        b.handle = Some(h);
                        self.overlay.set_acting(true);
                        let _ = self.events.send(ControlEvent::Acting { on: true });
                    }
                    Err(e) => {
                        return Err(format!(
                            "The Esc emergency stop could not start ({e}), so Deskhand will not take control of input."
                        ));
                    }
                }
            }
            b.generation += 1;
            self.active.fetch_add(1, Ordering::SeqCst);
        }
        Ok(ActionGuard { control: self.me.upgrade().expect("control alive"), generation: self.cancel_gen.load(Ordering::SeqCst) })
    }

    /// The user pressed Esc. Runs on the tap thread: must not block or drop
    /// the tap handle here.
    pub fn emergency_stop(&self) {
        self.cancel_gen.fetch_add(1, Ordering::SeqCst);
        *self.halted.lock().unwrap() = Some(Instant::now());
        self.cancel_notify.notify_waiters();
        let _ = self.events.send(ControlEvent::EmergencyStop);
        let holder = self.lock_holder().unwrap_or_else(|| "-".into());
        self.activity.record(&holder, "stop", "Esc", "user pressed Esc: cancelled the action in flight and halted", true);
        tracing::warn!("emergency stop (Esc)");
        let handle = self.burst.lock().unwrap().handle.take();
        if handle.is_some() {
            self.overlay.set_acting(false);
            let _ = self.events.send(ControlEvent::Acting { on: false });
            // Dropping the handle joins the tap thread we may be on.
            std::thread::spawn(move || drop(handle));
        }
    }

    /// The user sent a new prompt (or the turn ended): lift the Esc halt.
    pub fn turn_ended(&self) {
        *self.halted.lock().unwrap() = None;
    }

    pub fn is_halted(&self) -> bool {
        self.halted.lock().unwrap().is_some_and(|t| t.elapsed() < HALT_FALLBACK)
    }

    pub fn lock_holder(&self) -> Option<String> {
        let l = self.lock.lock().unwrap();
        l.holder.clone().filter(|_| l.last_used.elapsed() < LOCK_STALE)
    }

    /// A session ended: release its lock.
    pub fn release(&self, session: &str) {
        let mut l = self.lock.lock().unwrap();
        if l.holder.as_deref() == Some(session) {
            l.holder = None;
            drop(l);
            self.activity.record(session, "lock", "", "released the input lock (session ended)", true);
        }
    }

    pub fn set_approval_pending(&self, pending: bool) {
        self.approval_pending.store(pending, Ordering::SeqCst);
    }

    pub fn approval_pending(&self) -> bool {
        self.approval_pending.load(Ordering::SeqCst)
    }

    /// True while at least one input action is running.
    pub fn in_flight(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }

    pub fn is_armed(&self) -> bool {
        self.burst.lock().unwrap().handle.is_some()
    }

    fn action_done(&self) {
        let generation = {
            let b = self.burst.lock().unwrap();
            b.generation
        };
        if let Ok(mut l) = self.lock.lock() {
            l.last_used = Instant::now();
        }
        if self.active.fetch_sub(1, Ordering::SeqCst) != 1 {
            return;
        }
        let me = self.me.clone();
        let idle = self.idle;
        std::thread::spawn(move || {
            std::thread::sleep(idle);
            if let Some(c) = me.upgrade() {
                c.end_burst_if(generation);
            }
        });
    }

    fn end_burst_if(&self, generation: u64) {
        let handle = {
            let mut b = self.burst.lock().unwrap();
            if b.generation != generation || self.active.load(Ordering::SeqCst) != 0 {
                return;
            }
            b.handle.take()
        };
        if handle.is_some() {
            self.overlay.set_acting(false);
            let _ = self.events.send(ControlEvent::Acting { on: false });
            drop(handle);
        }
    }

    /// Disarm now (app shutdown).
    pub fn shutdown(&self) {
        let handle = self.burst.lock().unwrap().handle.take();
        if handle.is_some() {
            self.overlay.set_acting(false);
        }
        drop(handle);
    }
}

/// One running action. Dropping it ends the action.
pub struct ActionGuard {
    control: Arc<Control>,
    generation: u64,
}

impl ActionGuard {
    /// The user pressed Esc since this action started.
    pub fn is_cancelled(&self) -> bool {
        self.control.cancel_gen.load(Ordering::SeqCst) != self.generation
    }

    /// Resolves when the user presses Esc.
    pub async fn cancelled(&self) {
        loop {
            let n = self.control.cancel_notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }

    /// A `should_stop` closure for blocking input (drag, long typing).
    pub fn stop_flag(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let c = self.control.clone();
        let g = self.generation;
        move || c.cancel_gen.load(Ordering::SeqCst) != g
    }
}

impl Drop for ActionGuard {
    fn drop(&mut self) {
        self.control.action_done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::{MockOverlay, MockStopKey};

    fn control(idle_ms: u64) -> (Arc<Control>, Arc<MockStopKey>, Arc<MockOverlay>) {
        let sk = Arc::new(MockStopKey::default());
        let ov = Arc::new(MockOverlay::default());
        let c = Control::new(sk.clone(), ov.clone(), Arc::new(ActivityLog::memory()), Duration::from_millis(idle_ms));
        (c, sk, ov)
    }

    #[test]
    fn one_session_holds_the_lock() {
        let (c, _, _) = control(10_000);
        let g = c.begin_action("aaaa").unwrap();
        drop(g);
        let err = c.begin_action("bbbb").err().unwrap();
        assert!(err.contains("aaaa"), "{err}");
        assert!(c.begin_action("aaaa").is_ok());
        c.release("aaaa");
        assert!(c.begin_action("bbbb").is_ok());
        assert_eq!(c.lock_holder().as_deref(), Some("bbbb"));
    }

    #[test]
    fn esc_cancels_and_halts_until_next_prompt() {
        let (c, sk, ov) = control(10_000);
        let g = c.begin_action("s").unwrap();
        assert!(sk.is_armed());
        assert_eq!(*ov.acting.lock().unwrap(), vec![true]);
        assert!(!g.is_cancelled());
        assert!(sk.press());
        assert!(g.is_cancelled());
        let stop = g.stop_flag();
        assert!(stop());
        drop(g);
        assert!(c.begin_action("s").err().unwrap().contains("Esc"));
        c.turn_ended();
        let g2 = c.begin_action("s").unwrap();
        assert!(!g2.is_cancelled());
    }

    #[test]
    fn tap_disarms_after_idle() {
        let (c, sk, ov) = control(30);
        let g = c.begin_action("s").unwrap();
        drop(g);
        assert!(sk.is_armed());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!sk.is_armed());
        assert!(!c.is_armed());
        assert_eq!(*ov.acting.lock().unwrap(), vec![true, false]);
        // Esc now reaches apps normally.
        assert!(!sk.press());
    }

    #[test]
    fn approval_prompt_blocks_actions() {
        let (c, _, _) = control(10_000);
        c.set_approval_pending(true);
        assert!(c.begin_action("s").err().unwrap().contains("approval"));
        c.set_approval_pending(false);
        assert!(c.begin_action("s").is_ok());
    }

    #[tokio::test]
    async fn esc_wakes_a_waiting_action() {
        let (c, sk, _) = control(10_000);
        let g = c.begin_action("s").unwrap();
        let sk2 = sk.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            sk2.press();
        });
        let r = tokio::time::timeout(Duration::from_secs(2), g.cancelled()).await;
        assert!(r.is_ok(), "cancelled() should resolve on Esc");
    }

    struct FailingStopKey;
    impl StopKey for FailingStopKey {
        fn start(&self, _: Arc<dyn Fn() + Send + Sync>) -> crate::platform::Result<ListenerHandle> {
            Err(crate::platform::PlatformError::PermissionMissing(crate::platform::Permission::InputMonitoring))
        }
    }

    #[test]
    fn no_action_without_the_stop_key() {
        let c = Control::new(
            Arc::new(FailingStopKey),
            Arc::new(MockOverlay::default()),
            Arc::new(ActivityLog::memory()),
            DEFAULT_IDLE,
        );
        let err = c.begin_action("s").err().unwrap();
        assert!(err.contains("Esc emergency stop could not start"), "{err}");
    }
}
