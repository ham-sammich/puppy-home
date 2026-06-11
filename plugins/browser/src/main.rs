//! puppy-browser — the optional in-app browser plugin for puppy-home.
//!
//! It opens an OS-native webview (via `wry` on a `tao` event loop) and is driven
//! by the host over **stdin**, one JSON command per line:
//!
//! ```text
//! {"cmd":"navigate","url":"https://example.com"}
//! {"cmd":"back"} | {"cmd":"forward"} | {"cmd":"reload"} | {"cmd":"close"}
//! ```
//!
//! Keeping the webview in this separate process means a browser crash can't take
//! the host IDE down with it, and the host stays free of heavy webview deps.
//!
//! Release builds are a GUI app (no console window of their own).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::BufRead;

use serde::Deserialize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

/// One command from the host (line-delimited JSON on stdin).
#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
enum WireCmd {
    Navigate {
        url: String,
    },
    Back,
    Forward,
    Reload,
    /// Open the F12 DevTools window.
    Devtools,
    Close,
}

fn main() -> wry::Result<()> {
    let event_loop = EventLoopBuilder::<WireCmd>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Read host commands off stdin on a background thread; forward to the loop.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<WireCmd>(line) {
                Ok(cmd) => {
                    if proxy.send_event(cmd).is_err() {
                        break; // event loop gone
                    }
                }
                Err(e) => eprintln!("puppy-browser: bad command {line:?}: {e}"),
            }
        }
        // stdin closed (host went away) -> ask the loop to exit.
        let _ = proxy.send_event(WireCmd::Close);
    });

    let initial_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());
    // Optional 2nd arg: a Chrome DevTools Protocol remote-debugging port, so the
    // host (and Code Puppy) can attach to this page over CDP at 127.0.0.1:PORT.
    let cdp_port: Option<u16> = std::env::args().nth(2).and_then(|s| s.parse().ok());

    // Borderless: the host reparents this window into the Browser tab. On
    // Windows we start hidden and let the host show it once embedded (no flash);
    // elsewhere we stay a normal visible window (no embedding yet).
    let window = WindowBuilder::new()
        .with_title("Puppy Browser")
        .with_inner_size(tao::dpi::LogicalSize::new(1024.0, 720.0))
        .with_decorations(!cfg!(windows))
        .with_visible(!cfg!(windows))
        .build(&event_loop)
        .expect("create window");

    // `mut` is only needed on Windows (CDP args reassign below); silence the
    // unused-mut lint elsewhere so the plugin builds warning-free everywhere.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut builder = WebViewBuilder::new(&window)
        .with_url(&initial_url)
        .with_devtools(true);
    #[cfg(windows)]
    if let Some(port) = cdp_port {
        use wry::WebViewBuilderExtWindows;
        builder = builder.with_additional_browser_args(format!("--remote-debugging-port={port}"));
    }
    #[cfg(not(windows))]
    let _ = cdp_port;
    let webview = builder.build()?;

    // Tell the host our native window handle so it can embed us.
    report_handle(&window);
    eprintln!("puppy-browser: ready");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
                WireCmd::Navigate { url } => {
                    let _ = webview.load_url(&url);
                }
                WireCmd::Back => {
                    let _ = webview.evaluate_script("history.back()");
                }
                WireCmd::Forward => {
                    let _ = webview.evaluate_script("history.forward()");
                }
                WireCmd::Reload => {
                    let _ = webview.evaluate_script("location.reload()");
                }
                WireCmd::Devtools => webview.open_devtools(),
                WireCmd::Close => *control_flow = ControlFlow::Exit,
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    });
}

/// Print our native window handle so the host can reparent us. One JSON line:
/// `{"event":"hwnd","hwnd":<isize>}`. Only Windows embedding is wired today.
fn report_handle(window: &tao::window::Window) {
    use std::io::Write;
    #[cfg(windows)]
    let handle: i64 = {
        use tao::platform::windows::WindowExtWindows;
        window.hwnd() as isize as i64
    };
    #[cfg(not(windows))]
    let handle: i64 = {
        let _ = window;
        0
    };
    println!("{{\"event\":\"hwnd\",\"hwnd\":{handle}}}");
    let _ = std::io::stdout().flush();
}
