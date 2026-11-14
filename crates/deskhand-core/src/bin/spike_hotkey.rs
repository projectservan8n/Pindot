//! Spike (a): hold Fn, get down/up, and the OS never sees the press.
//!
//!   spike_hotkey status                        permissions this process has
//!   spike_hotkey listen [--pass-through] [--seconds N]
//!                                              print Fn down/up from the real keyboard
//!   spike_hotkey selftest [--control]          synthetic, no keyboard needed
//!   spike_hotkey chords [--target-pid PID]     synthetic rebinding test (Right Option,
//!                                              Ctrl+Cmd, cancel, Fn swallowed)
//!
//! selftest: a listen-only "watcher" tap sits after the session taps, where
//! the OS and apps would get the event. We post synthetic Fn down/up (marked
//! so we can tell them from a real key) and check that our swallowing tap
//! reports Down/Up while the watcher sees nothing. `--control` repeats it
//! with a pass-through tap, where the watcher must see both events; that
//! delivers a bare Fn press to the frontmost app, so only use it while your
//! own test window is in front.
//!
//! chords: the same idea for the rebindable chords a non-Apple keyboard
//! needs. Each case starts a tap for one binding, posts synthetic modifier
//! changes (marked, from a private source state, always released again) and
//! compares what the tap reported with what it should. Modifier-only presses
//! do nothing in apps. The one case that presses a real key (F13, to show
//! that any other key cancels a hold) only runs while the window with
//! `--target-pid` (spikes/test-target) is frontmost, so the key can only
//! reach that window.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use deskhand_core::platform::macos::hotkey::{MacHotkey, WatchedFn, post_synthetic_fn, watch_fn};
    use deskhand_core::platform::{HotkeyEvent, permission_report};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("status");
    let flag = |f: &str| args.iter().any(|a| a == f);
    let seconds: u64 = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    println!("pid {}", std::process::id());
    for p in permission_report() {
        println!("permission {:?}: {}", p.permission, if p.granted { "granted" } else { "MISSING" });
    }
    let fn_usage = std::process::Command::new("defaults")
        .args(["read", "com.apple.HIToolbox", "AppleFnUsageType"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unset (OS default)".into());
    println!("AppleFnUsageType (\"Press Globe key to\"): {fn_usage}");

    let drain_hot = |rx: &mpsc::Receiver<HotkeyEvent>| -> Vec<HotkeyEvent> { rx.try_iter().collect() };
    let drain_watch =
        |rx: &mpsc::Receiver<WatchedFn>| -> Vec<WatchedFn> { rx.try_iter().filter(|w| w.synthetic).collect() };
    let press = || -> anyhow::Result<()> {
        post_synthetic_fn(true)?;
        std::thread::sleep(Duration::from_millis(250));
        post_synthetic_fn(false)?;
        std::thread::sleep(Duration::from_millis(400));
        Ok(())
    };

    match mode {
        "status" => {}
        "listen" => {
            let swallow = !flag("--pass-through");
            let (tx, rx) = mpsc::channel();
            let handle = MacHotkey.start_fn(tx, swallow)?;
            println!("listening for {seconds}s (swallow={swallow}); hold and release Fn");
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end {
                if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
                    println!("{:.3} Fn {:?}", std::time::UNIX_EPOCH.elapsed()?.as_secs_f64(), ev);
                }
            }
            handle.stop();
            println!("tap stopped");
        }
        "selftest" => {
            let (wtx, wrx) = mpsc::channel();
            let watcher = watch_fn(wtx)?;
            println!("watcher tap (listen-only, annotated session level) running");

            // Swallow run.
            let (tx, rx) = mpsc::channel();
            let tap = MacHotkey.start_fn(tx, true)?;
            press()?;
            let ours = drain_hot(&rx);
            let seen = drain_watch(&wrx);
            tap.stop();
            println!("SWALLOW run: our tap reported {ours:?}; watcher saw {} synthetic Fn events", seen.len());
            let swallow_ok = ours == [HotkeyEvent::Down, HotkeyEvent::Up] && seen.is_empty();

            let mut control_ok = None;
            if flag("--control") {
                let (tx, rx) = mpsc::channel();
                let tap = MacHotkey.start_fn(tx, false)?;
                press()?;
                let ours = drain_hot(&rx);
                let seen = drain_watch(&wrx);
                tap.stop();
                println!(
                    "CONTROL run (pass-through): our tap reported {ours:?}; watcher saw {:?}",
                    seen.iter().map(|w| if w.down { "down" } else { "up" }).collect::<Vec<_>>()
                );
                control_ok = Some(ours == [HotkeyEvent::Down, HotkeyEvent::Up] && seen.len() == 2);
            }
            watcher.stop();
            println!("taps stopped");
            let pass = swallow_ok && control_ok.unwrap_or(true);
            println!(
                "RESULT {} (swallow {}, control {})",
                if pass { "PASS" } else { "FAIL" },
                if swallow_ok { "ok" } else { "failed" },
                match control_ok {
                    Some(true) => "ok",
                    Some(false) => "failed",
                    None => "not run",
                }
            );
            if !pass {
                std::process::exit(1);
            }
        }
        "chords" => {
            let target_pid: Option<i32> = args
                .iter()
                .position(|a| a == "--target-pid")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse().ok());
            if !chords(target_pid)? {
                std::process::exit(1);
            }
        }
        other => anyhow::bail!("unknown mode {other}; use status, listen, selftest or chords"),
    }
    Ok(())
}

/// One step of a synthetic chord case.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum Step {
    /// flagsChanged for this keycode with this full modifier state.
    Flags(u16, u64),
    /// A key down/up (F13) with this modifier state.
    Key(u16, bool, u64),
    Wait(u64),
}

#[cfg(target_os = "macos")]
fn chords(target_pid: Option<i32>) -> anyhow::Result<bool> {
    use std::sync::mpsc;
    use std::time::Duration;

    use deskhand_core::platform::macos::hotkey::*;
    use deskhand_core::platform::{HotkeyBinding, HotkeyEvent, ModKey, Side};
    use Step::*;

    const KVK_F13: u16 = 0x69;
    let f = mac_flags_for;
    let ro = f(ModKey::Option, Side::Right);
    let lo = f(ModKey::Option, Side::Left);
    let ctrl = f(ModKey::Control, Side::Left);
    let cmd = f(ModKey::Command, Side::Left);
    let shift = f(ModKey::Shift, Side::Left);
    let fnf = f(ModKey::Fn, Side::Either);
    let fnk = KVK_FUNCTION as u16;
    let hold = Wait(250);

    struct Case {
        name: &'static str,
        binding: &'static str,
        steps: Vec<Step>,
        expect: Vec<HotkeyEvent>,
        /// Synthetic flagsChanged events that must reach apps (None = not checked).
        reach_apps: Option<usize>,
        needs_target: bool,
    }
    use HotkeyEvent::{Cancel, Down, Up};
    let cases = vec![
        Case {
            name: "Right Option: hold and release",
            binding: "Right Option",
            steps: vec![Flags(KVK_RIGHT_OPTION, ro), hold, Flags(KVK_RIGHT_OPTION, 0)],
            expect: vec![Down, Up],
            reach_apps: Some(2),
            needs_target: false,
        },
        Case {
            name: "Right Option: Left Option is not a match",
            binding: "Right Option",
            steps: vec![Flags(KVK_LEFT_OPTION, lo), hold, Flags(KVK_LEFT_OPTION, 0)],
            expect: vec![],
            reach_apps: Some(2),
            needs_target: false,
        },
        Case {
            name: "Ctrl+Cmd: press both, release",
            binding: "Ctrl+Cmd",
            steps: vec![
                Flags(KVK_LEFT_CONTROL, ctrl),
                Flags(KVK_LEFT_COMMAND, ctrl | cmd),
                hold,
                Flags(KVK_LEFT_COMMAND, ctrl),
                Flags(KVK_LEFT_CONTROL, 0),
            ],
            expect: vec![Down, Up],
            reach_apps: Some(4),
            needs_target: false,
        },
        Case {
            name: "Ctrl+Cmd: Ctrl alone does nothing",
            binding: "Ctrl+Cmd",
            steps: vec![Flags(KVK_LEFT_CONTROL, ctrl), hold, Flags(KVK_LEFT_CONTROL, 0)],
            expect: vec![],
            reach_apps: None,
            needs_target: false,
        },
        Case {
            name: "Ctrl+Cmd: Shift joins, the hold is cancelled and stays off",
            binding: "Ctrl+Cmd",
            steps: vec![
                Flags(KVK_LEFT_CONTROL, ctrl),
                Flags(KVK_LEFT_COMMAND, ctrl | cmd),
                hold,
                Flags(KVK_LEFT_SHIFT, ctrl | cmd | shift),
                Flags(KVK_LEFT_SHIFT, ctrl | cmd),
                hold,
                Flags(KVK_LEFT_COMMAND, ctrl),
                Flags(KVK_LEFT_CONTROL, 0),
            ],
            expect: vec![Down, Cancel],
            reach_apps: None,
            needs_target: false,
        },
        Case {
            name: "Right Option + F13 key: cancelled",
            binding: "Right Option",
            steps: vec![
                Flags(KVK_RIGHT_OPTION, ro),
                hold,
                Key(KVK_F13, true, ro),
                Key(KVK_F13, false, ro),
                Flags(KVK_RIGHT_OPTION, 0),
            ],
            expect: vec![Down, Cancel],
            reach_apps: None,
            needs_target: true,
        },
        Case {
            name: "Fn: reported and swallowed (apps never see it)",
            binding: "Fn",
            steps: vec![Flags(fnk, fnf), hold, Flags(fnk, 0)],
            expect: vec![Down, Up],
            reach_apps: Some(0),
            needs_target: false,
        },
    ];

    let front_is_target = || {
        let Some(want) = target_pid else { return false };
        let ax = deskhand_core::platform::native().accessibility;
        ax.frontmost_pid().ok().flatten() == Some(want)
    };

    let (wtx, wrx) = mpsc::channel();
    let watcher = watch_fn(wtx)?;
    let mut all_ok = true;
    for c in &cases {
        if c.needs_target && !front_is_target() {
            println!("CASE {:<58} SKIPPED (test window not frontmost; pass --target-pid)", c.name);
            all_ok = false;
            continue;
        }
        let binding: HotkeyBinding = c.binding.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
        let (tx, rx) = mpsc::channel();
        let tap = MacHotkey.start_chord(binding.clone(), tx, true)?;
        let _ = wrx.try_iter().count();
        let mut posted = Vec::new();
        let mut err = None;
        for s in &c.steps {
            let r = match *s {
                Flags(k, fl) => {
                    posted.push(k);
                    post_synthetic_flags(k, fl)
                }
                Key(k, down, fl) => post_synthetic_key(k, down, fl),
                Wait(ms) => {
                    std::thread::sleep(Duration::from_millis(ms));
                    Ok(())
                }
            };
            if let Err(e) = r {
                err = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        if err.is_some() {
            // Never leave a synthetic modifier down.
            for k in posted {
                let _ = post_synthetic_flags(k, 0);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        let got: Vec<HotkeyEvent> = rx.try_iter().collect();
        let seen = wrx.try_iter().filter(|w| w.synthetic).count();
        tap.stop();
        let events_ok = got == c.expect;
        let reach_ok = c.reach_apps.is_none_or(|n| n == seen);
        let ok = err.is_none() && events_ok && reach_ok;
        all_ok &= ok;
        println!(
            "CASE {:<58} {} binding {:<13} tap reported {:?} (want {:?}); synthetic modifier events that reached apps: {}{}",
            c.name,
            if ok { "PASS" } else { "FAIL" },
            binding.label_for(deskhand_core::platform::Os::MacOs),
            got,
            c.expect,
            seen,
            c.reach_apps.map(|n| format!(" (want {n})")).unwrap_or_default()
        );
        if let Some(e) = err {
            println!("     post failed: {e}");
        }
    }
    watcher.stop();
    println!("taps stopped");
    println!("RESULT {}", if all_ok { "PASS" } else { "FAIL" });
    Ok(all_ok)
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("spike_hotkey is macOS only; the Windows hook is a stub (see docs/windows-test-checklist.md)");
}
