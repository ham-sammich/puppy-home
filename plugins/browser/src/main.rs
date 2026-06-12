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
    /// macOS in-tab embedding: position this borderless window over the host's
    /// Browser tab (physical screen px, top-left origin) and z-order it just
    /// above the host window (`parent` is the host's NSWindow number).
    Embed {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: i64,
    },
    /// macOS embedding: order this window out (tab inactive / hidden).
    Hide,
    /// Floating-window mode: show as a normal decorated window (hosts that
    /// can't embed — the GPUI shell). Embeddable platforms start hidden, so
    /// without this (or `embed`) the window would never appear at all.
    Float,
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

    // Platforms that embed the webview into the host's Browser tab (Windows
    // reparents; macOS overlays a borderless window the host positions) start
    // borderless + hidden so there's no flash before the host places us. Other
    // platforms (Linux) stay a normal visible window (no embedding yet).
    let embeddable = cfg!(windows) || cfg!(target_os = "macos");
    let window = WindowBuilder::new()
        .with_title("Puppy Browser")
        .with_inner_size(tao::dpi::LogicalSize::new(1024.0, 720.0))
        .with_decorations(!embeddable)
        .with_visible(!embeddable)
        .build(&event_loop)
        .expect("create window");

    // `mut` is only needed on Windows (CDP args reassign below); silence the
    // unused-mut lint elsewhere so the plugin builds warning-free everywhere.
    // A modern, mainstream User-Agent. wry's default UA is an anonymous
    // WebKit string many sites sniff as "unknown old browser" and serve
    // legacy/degraded layouts to — the engine is evergreen, the DEFAULT
    // UA is what looked dated. Safari freezes the OS token at 10_15_7 by
    // design; matching real Safari exactly is the point.
    let user_agent = if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/17.6 Safari/605.1.15"
    } else if cfg!(windows) {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
    };
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut builder = WebViewBuilder::new(&window)
        .with_url(&initial_url)
        .with_user_agent(user_agent)
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
                WireCmd::Embed { x, y, w, h, parent } => {
                    // Re-embedding after a pop-out (`float`) must shed the
                    // decorations again; idempotent when already borderless.
                    window.set_decorations(false);
                    window.set_outer_position(tao::dpi::PhysicalPosition::new(x, y));
                    window.set_inner_size(tao::dpi::PhysicalSize::new(
                        w.max(1) as u32,
                        h.max(1) as u32,
                    ));
                    #[cfg(target_os = "macos")]
                    mac_order_above(&window, parent);
                    #[cfg(not(target_os = "macos"))]
                    let _ = parent;
                }
                WireCmd::Hide => {
                    #[cfg(target_os = "macos")]
                    mac_order_out(&window);
                    #[cfg(not(target_os = "macos"))]
                    window.set_visible(false);
                }
                WireCmd::Float => {
                    window.set_decorations(true);
                    window.set_visible(true);
                    // Detach visually from the embed spot: left glued in
                    // place, the floating window covered the host's browser
                    // toolbar — hiding the pop-in and Stop buttons (the
                    // "can't pop back in / can't close" reports). A real
                    // pop-out moves to a standalone position and size.
                    window.set_outer_position(tao::dpi::LogicalPosition::new(140.0, 120.0));
                    window.set_inner_size(tao::dpi::LogicalSize::new(1100.0, 740.0));
                }
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

/// Order our borderless window just above the host window (`parent` = the
/// host's NSWindow number). This also makes us visible without stealing key
/// focus from the host (unlike `orderFront`/`makeKeyAndOrderFront`).
#[cfg(target_os = "macos")]
fn mac_order_above(window: &tao::window::Window, parent: i64) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use tao::platform::macos::WindowExtMacOS;
    let ns_window = window.ns_window() as *mut Object;
    if ns_window.is_null() {
        return;
    }
    // NSWindowOrderingMode::NSWindowAbove == 1.
    unsafe {
        let _: () = msg_send![ns_window, orderWindow: 1isize relativeTo: parent as isize];
    }
}

/// Order our window out of the screen list (hidden while its tab is inactive).
#[cfg(target_os = "macos")]
fn mac_order_out(window: &tao::window::Window) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use tao::platform::macos::WindowExtMacOS;
    let ns_window = window.ns_window() as *mut Object;
    if ns_window.is_null() {
        return;
    }
    let nil: *mut Object = std::ptr::null_mut();
    unsafe {
        let _: () = msg_send![ns_window, orderOut: nil];
    }
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
