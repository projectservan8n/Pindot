//! Deskhand app, Phase 0: one click-through AI-cursor overlay per monitor,
//! plus the embedded MCP server whose click tool drives that cursor.
//!
//!   deskhand [--demo] [--exit-after SECONDS] [--no-server] [--capturable]
//!
//! `--demo` flies the cursor to the centre and corners of every monitor.
//! `--capturable` lets screenshots see the cursor (for tests only).

mod overlay;

use std::sync::Arc;
use std::time::Duration;

use deskhand_core::brain::config::{write_hook_settings, write_mcp_config};
use deskhand_core::paths::load_or_create_token;
use deskhand_core::platform::{Overlay, Point, native};
use deskhand_core::server::{ServerState, start};
use tauri::Manager;

pub use overlay::{MonitorRect, TauriOverlay};

struct Args {
    demo: bool,
    exit_after: Option<u64>,
    server: bool,
    capturable: bool,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    Args {
        demo: a.iter().any(|x| x == "--demo"),
        exit_after: a.iter().position(|x| x == "--exit-after").and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()),
        server: !a.iter().any(|x| x == "--no-server"),
        capturable: a.iter().any(|x| x == "--capturable"),
    }
}

pub fn run() {
    deskhand_core::init_logging();
    let args = parse_args();

    let builder = tauri::Builder::default();
    // Tauri activates the app at launch by default; an overlay must never
    // take focus from the app the user is working in.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init()).activate_ignoring_other_apps(false);

    builder
        .setup(move |app| {
            // No Dock icon, no menu bar, never steals focus.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let monitors: Vec<MonitorRect> = app
                .available_monitors()?
                .iter()
                .map(|m| {
                    let s = m.scale_factor();
                    let p = m.position().to_logical::<f64>(s);
                    let z = m.size().to_logical::<f64>(s);
                    MonitorRect { x: p.x, y: p.y, width: z.width, height: z.height, scale: s }
                })
                .collect();
            for (i, m) in monitors.iter().enumerate() {
                println!("monitor {i}: origin ({}, {}) size {}x{} pt, scale {}", m.x, m.y, m.width, m.height, m.scale);
                overlay::create_panel(app.handle(), i, m, args.capturable)?;
            }
            let ov = Arc::new(TauriOverlay::new(app.handle().clone()));
            app.manage(ov.clone());

            if args.server {
                let platform = native();
                let state = ServerState::new(platform.capture.clone(), platform.input.clone(), ov.clone());
                let token = load_or_create_token()?;
                let port = deskhand_core::port_from_env();
                let server = tauri::async_runtime::block_on(start(state, &token, port))?;
                write_mcp_config(&server.mcp_url(), &token)?;
                write_hook_settings(&server.stop_hook_url(), &token)?;
                println!("MCP server: {} (token and configs in the Deskhand data dir)", server.mcp_url());
                app.manage(server);
            }

            if args.demo {
                let ov = ov.clone();
                let monitors = monitors.clone();
                std::thread::spawn(move || run_demo(&*ov, &monitors));
            }
            if let Some(secs) = args.exit_after {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(secs));
                    println!("exit-after {secs}s reached; quitting");
                    handle.exit(0);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Deskhand")
        .run(|_app, event| {
            if let tauri::RunEvent::Ready = event {
                #[cfg(target_os = "macos")]
                overlay::log_activation_policy();
            }
        });
}

/// Centre, then each corner (inset 60 pt), then centre, on every monitor.
fn run_demo(ov: &dyn Overlay, monitors: &[MonitorRect]) {
    std::thread::sleep(Duration::from_millis(1500)); // let the webviews load
    for (i, m) in monitors.iter().enumerate() {
        let inset = 60.0;
        let points = [
            ("centre", m.x + m.width / 2.0, m.y + m.height / 2.0),
            ("top-left", m.x + inset, m.y + inset),
            ("top-right", m.x + m.width - inset, m.y + inset),
            ("bottom-right", m.x + m.width - inset, m.y + m.height - inset),
            ("bottom-left", m.x + inset, m.y + m.height - inset),
            ("centre", m.x + m.width / 2.0, m.y + m.height / 2.0),
        ];
        for (name, x, y) in points {
            let d = ov.move_cursor(Point::new(x, y), Some(format!("demo: monitor {i} {name}")));
            let t = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs_f64()).unwrap_or(0.0);
            println!("{t:.3} demo: monitor {i} {name} -> ({x:.0}, {y:.0}) over {} ms", d.as_millis());
            std::thread::sleep(d + Duration::from_millis(1200));
        }
    }
    println!("demo done");
}
