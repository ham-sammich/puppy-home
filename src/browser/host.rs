//! A supervised browser plugin process: spawn the standalone `puppy-browser`
//! executable and drive it over stdin with line-delimited JSON commands.
//!
//! This mirrors the Code Puppy sidecar pattern — the heavy webview lives in its
//! own process, so a browser crash never takes the IDE down.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use serde_json::json;

/// A running browser plugin process.
pub struct BrowserHost {
    child: Child,
    stdin: Option<ChildStdin>,
    /// The plugin's native window handle (0 until it reports one over stdout).
    hwnd: Arc<AtomicI64>,
    /// Set once the plugin reports its window exists (the `hwnd` event). On
    /// macOS the handle value is 0, so this flag — not `hwnd` — gates embedding.
    ready: Arc<AtomicBool>,
}

impl BrowserHost {
    /// Launch the plugin executable, opening `initial_url` on start. When
    /// `cdp_port` is set, the page exposes a CDP remote-debugging endpoint there.
    pub fn spawn(exe: &Path, initial_url: &str, cdp_port: Option<u16>) -> std::io::Result<Self> {
        let mut cmd = Command::new(exe);
        crate::proc::hide_console(&mut cmd);
        cmd.arg(initial_url);
        if let Some(port) = cdp_port {
            cmd.arg(port.to_string());
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            // stdout carries events (e.g. the window handle); stderr is logs.
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        let hwnd = Arc::new(AtomicI64::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        if let Some(out) = child.stdout.take() {
            let hwnd = hwnd.clone();
            let ready = ready.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(h) = parse_hwnd(&line) {
                        hwnd.store(h, Ordering::Relaxed);
                        ready.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            hwnd,
            ready,
        })
    }

    /// The plugin's native window handle, once reported.
    pub fn child_hwnd(&self) -> Option<i64> {
        match self.hwnd.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        }
    }

    /// Whether the plugin has reported its window exists (ready to be placed).
    /// Used on macOS, where the handle value is 0 but the window is real.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// macOS embedding: position the borderless plugin window over the host's
    /// Browser tab (physical screen px, top-left origin) and z-order it just
    /// above the host window (`parent` = the host's NSWindow number).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn embed(&mut self, x: i32, y: i32, w: i32, h: i32, parent: i64) {
        self.send(
            json!({ "cmd": "embed", "x": x, "y": y, "w": w, "h": h, "parent": parent }).to_string(),
        );
    }

    /// macOS embedding: order the plugin window out (its tab is inactive).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn hide(&mut self) {
        self.send(json!({ "cmd": "hide" }).to_string());
    }

    /// Floating-window mode: ask the plugin to show itself as a normal
    /// decorated window. Hosts that can't embed (the GPUI shell) MUST send
    /// this once the plugin is ready — embeddable platforms start hidden,
    /// so otherwise no window ever appears (the E8 macOS bug).
    pub fn float(&mut self) {
        self.send(json!({ "cmd": "float" }).to_string());
    }

    /// Write one JSON command line to the plugin.
    fn send(&mut self, line: String) {
        if let Some(w) = self.stdin.as_mut() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }

    /// Navigate to a URL.
    pub fn navigate(&mut self, url: &str) {
        self.send(json!({ "cmd": "navigate", "url": url }).to_string());
    }

    /// Go back in history.
    pub fn back(&mut self) {
        self.send(json!({ "cmd": "back" }).to_string());
    }

    /// Go forward in history.
    pub fn forward(&mut self) {
        self.send(json!({ "cmd": "forward" }).to_string());
    }

    /// Reload the current page.
    pub fn reload(&mut self) {
        self.send(json!({ "cmd": "reload" }).to_string());
    }

    /// Open the F12 DevTools window.
    pub fn devtools(&mut self) {
        self.send(json!({ "cmd": "devtools" }).to_string());
    }

    /// Whether the process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the plugin to close, then ensure the process is gone.
    pub fn close(&mut self) {
        self.send(json!({ "cmd": "close" }).to_string());
        let _ = self.child.kill();
    }
}

impl Drop for BrowserHost {
    fn drop(&mut self) {
        // A dropped host (closed tab / app exit) must not leave a zombie window.
        let _ = self.child.kill();
    }
}

/// Parse a `{"event":"hwnd","hwnd":<isize>}` line into the handle.
fn parse_hwnd(line: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("event")?.as_str()? == "hwnd" {
        v.get("hwnd")?.as_i64()
    } else {
        None
    }
}
