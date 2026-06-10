//! A supervised browser plugin process: spawn the standalone `puppy-browser`
//! executable and drive it over stdin with line-delimited JSON commands.
//!
//! This mirrors the Code Puppy sidecar pattern — the heavy webview lives in its
//! own process, so a browser crash never takes the IDE down.

use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::json;

/// A running browser plugin process.
pub struct BrowserHost {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl BrowserHost {
    /// Launch the plugin executable, opening `initial_url` on start.
    pub fn spawn(exe: &Path, initial_url: &str) -> std::io::Result<Self> {
        let mut child = Command::new(exe)
            .arg(initial_url)
            .stdin(Stdio::piped())
            // Swallow the plugin's stdout/stderr (it logs to stderr); we only
            // talk to it via stdin for now.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
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
