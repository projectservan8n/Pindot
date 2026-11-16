//! Rate limit and lock for `vault_fill`, one per MCP session.
//!
//! * At most `MAX_FILLS` typed fills per `WINDOW`.
//! * `LOCK_AFTER` safety refusals (wrong site, not a browser, wrong kind of
//!   field) within `WINDOW` lock the vault for the rest of the session's
//!   `LOCK_FOR`: something is steering Rusty at the wrong page.
//!
//! Time is passed in, so tests need no sleeping.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const WINDOW: Duration = Duration::from_secs(60);
pub const MAX_FILLS: usize = 5;
pub const LOCK_AFTER: usize = 3;
pub const LOCK_FOR: Duration = Duration::from_secs(300);

#[derive(Default)]
struct State {
    fills: VecDeque<Instant>,
    refusals: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

#[derive(Default)]
pub struct FillGate {
    state: Mutex<State>,
}

fn trim(q: &mut VecDeque<Instant>, now: Instant) {
    while q.front().is_some_and(|t| now.duration_since(*t) >= WINDOW) {
        q.pop_front();
    }
}

impl FillGate {
    /// `Err(reason)` when a fill must not even be attempted now.
    pub fn check(&self, now: Instant) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        if let Some(until) = s.locked_until {
            if now < until {
                let mins = (until - now).as_secs().div_ceil(60);
                return Err(format!(
                    "the vault is locked for this session after repeated refusals (about {mins} min left); ask the user"
                ));
            }
            s.locked_until = None;
            s.refusals.clear();
        }
        trim(&mut s.fills, now);
        if s.fills.len() >= MAX_FILLS {
            return Err(format!("too many sign-in fills; at most {MAX_FILLS} per minute, wait and try again"));
        }
        Ok(())
    }

    /// A secret was typed.
    pub fn note_fill(&self, now: Instant) {
        let mut s = self.state.lock().unwrap();
        trim(&mut s.fills, now);
        s.fills.push_back(now);
    }

    /// A safety refusal. Returns true when this one locked the vault.
    pub fn note_refusal(&self, now: Instant) -> bool {
        let mut s = self.state.lock().unwrap();
        trim(&mut s.refusals, now);
        s.refusals.push_back(now);
        if s.refusals.len() >= LOCK_AFTER {
            s.locked_until = Some(now + LOCK_FOR);
            return true;
        }
        false
    }

    pub fn is_locked(&self, now: Instant) -> bool {
        self.state.lock().unwrap().locked_until.is_some_and(|u| now < u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_allows_five_per_minute() {
        let g = FillGate::default();
        let t0 = Instant::now();
        for i in 0..MAX_FILLS {
            let t = t0 + Duration::from_secs(i as u64);
            assert!(g.check(t).is_ok());
            g.note_fill(t);
        }
        let now = t0 + Duration::from_secs(10);
        assert!(g.check(now).unwrap_err().contains("too many"));
        // The oldest fill leaves the window after 60 s.
        assert!(g.check(t0 + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn three_refusals_lock_for_five_minutes() {
        let g = FillGate::default();
        let t0 = Instant::now();
        assert!(!g.note_refusal(t0));
        assert!(!g.note_refusal(t0 + Duration::from_secs(5)));
        assert!(g.check(t0 + Duration::from_secs(6)).is_ok());
        assert!(g.note_refusal(t0 + Duration::from_secs(10)));
        assert!(g.is_locked(t0 + Duration::from_secs(11)));
        assert!(g.check(t0 + Duration::from_secs(11)).unwrap_err().contains("locked"));
        assert!(g.check(t0 + Duration::from_secs(299)).is_err());
        assert!(g.check(t0 + Duration::from_secs(311)).is_ok());
        assert!(!g.is_locked(t0 + Duration::from_secs(311)));
    }

    #[test]
    fn spread_out_refusals_do_not_lock() {
        let g = FillGate::default();
        let t0 = Instant::now();
        for i in 0..6 {
            assert!(!g.note_refusal(t0 + Duration::from_secs(i * 40)));
        }
        assert!(g.check(t0 + Duration::from_secs(250)).is_ok());
    }

    #[test]
    fn a_new_gate_per_session_is_independent() {
        let (a, b) = (FillGate::default(), FillGate::default());
        let t0 = Instant::now();
        for _ in 0..LOCK_AFTER {
            a.note_refusal(t0);
        }
        assert!(a.check(t0).is_err());
        assert!(b.check(t0).is_ok());
    }
}
