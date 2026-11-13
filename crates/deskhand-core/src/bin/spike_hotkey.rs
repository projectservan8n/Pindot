//! Spike (a): hold Fn, get down/up, and the OS never sees the press.
//!
//!   spike_hotkey status                        permissions this process has
//!   spike_hotkey listen [--pass-through] [--seconds N]
//!                                              print Fn down/up from the real keyboard
//!   spike_hotkey selftest [--control]          synthetic, no keyboard needed
//!
//! selftest: a listen-only "watcher" tap sits after the session taps, where
//! the OS and apps would get the event. We post synthetic Fn down/up (marked
//! so we can tell them from a real key) and check that our swallowing tap
//! reports Down/Up while the watcher sees nothing. `--control` repeats it
//! with a pass-through tap, where the watcher must see both events; that
//! delivers a bare Fn press to the frontmost app, so only use it while your
//! own test window is in front.

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
        other => anyhow::bail!("unknown mode {other}; use status, listen or selftest"),
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("spike_hotkey is macOS only; the Windows hook is a stub (see docs/windows-test-checklist.md)");
}
