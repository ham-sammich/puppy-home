//! A full pseudo-terminal scoped to a workspace folder.
//!
//! Spawns the platform shell on a real PTY (ConPTY on Windows, openpty on
//! Unix), feeds its output through a `vt100` screen parser, and renders the
//! resulting cell grid with egui's painter — so colors, cursor movement, and
//! curses-style TUIs (vim, top, htop) work like a real terminal. Keyboard input
//! is translated to terminal byte sequences and written back to the PTY.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::waker::UiWaker;

const SCROLLBACK: usize = 5000;

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Does `haystack` contain the byte sequence `needle`?
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    // Keep the slave alive: on Windows it owns the ConPTY handle, so dropping it
    // tears down the pseudo-console and the shell's output stops flowing.
    _slave: Box<dyn portable_pty::SlavePty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    rows: u16,
    cols: u16,
    scrollback: usize,
    /// Grab keyboard focus the first time the grid is shown.
    pub alive: bool,
}

/// `(exe, args)` for the shell. Overridable via `PUPPY_HOME_SHELL`.
fn shell() -> (String, Vec<String>) {
    if let Ok(custom) = std::env::var("PUPPY_HOME_SHELL") {
        let mut parts = custom.split_whitespace();
        if let Some(exe) = parts.next() {
            return (exe.to_string(), parts.map(str::to_string).collect());
        }
    }
    #[cfg(windows)]
    {
        ("powershell.exe".to_string(), vec!["-NoLogo".to_string()])
    }
    #[cfg(not(windows))]
    {
        let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        (sh, vec!["-i".to_string()])
    }
}

impl Terminal {
    /// Spawn a local shell on a fresh PTY rooted at `cwd`. `waker` wakes the
    /// UI on output.
    pub fn spawn(cwd: &Path, waker: Arc<dyn UiWaker>) -> Result<Self, String> {
        let (exe, args) = shell();
        let mut cmd = CommandBuilder::new(&exe);
        for a in &args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        Self::spawn_cmd(cmd, waker)
    }

    /// Spawn a shell on the REMOTE host of an SSH workspace: interactive
    /// `ssh -t` runs inside the local PTY, lands in `remote_root`, and execs
    /// the login shell there (B13.7). The vt100/render stack is transport
    /// agnostic — ssh just carries the bytes, so auth prompts (password,
    /// 2FA) appear in the terminal like in any other ssh session, and an
    /// ssh exit surfaces as the regular dead-shell notice.
    pub fn spawn_remote(
        target: &crate::backend::ssh::SshTarget,
        remote_root: &str,
        waker: Arc<dyn UiWaker>,
    ) -> Result<Self, String> {
        let mut cmd = CommandBuilder::new("ssh");
        for a in target.terminal_args(remote_root) {
            cmd.arg(a);
        }
        Self::spawn_cmd(cmd, waker)
    }

    /// Shared PTY plumbing: open the pair, run `cmd` on the slave, wire the
    /// reader thread (vt100 + query replies + wake throttle).
    fn spawn_cmd(mut cmd: CommandBuilder, waker: Arc<dyn UiWaker>) -> Result<Self, String> {
        let (rows, cols) = (24u16, 80u16);
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: SharedWriter = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(|e| e.to_string())?,
        ));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));

        {
            let parser = parser.clone();
            let writer = writer.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut last_wake = std::time::Instant::now() - std::time::Duration::from_secs(1);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let chunk = &buf[..n];
                            // Process, then answer any terminal queries the program
                            // sent. ConPTY's conhost BLOCKS on the cursor-position
                            // report (ESC[6n) at startup — without a reply the shell
                            // prompt never renders.
                            let mut reply: Vec<u8> = Vec::new();
                            if let Ok(mut p) = parser.lock() {
                                p.process(chunk);
                                if contains(chunk, b"\x1b[6n") {
                                    let (r, c) = p.screen().cursor_position();
                                    reply.extend_from_slice(
                                        format!("\x1b[{};{}R", r + 1, c + 1).as_bytes(),
                                    );
                                }
                            }
                            if contains(chunk, b"\x1b[5n") {
                                reply.extend_from_slice(b"\x1b[0n"); // status: OK
                            }
                            if contains(chunk, b"\x1b[c") || contains(chunk, b"\x1b[0c") {
                                reply.extend_from_slice(b"\x1b[?1;2c"); // primary DA: VT100
                            }
                            if !reply.is_empty()
                                && let Ok(mut w) = writer.lock()
                            {
                                let _ = w.write_all(&reply);
                                let _ = w.flush();
                            }
                            // Wake throttle: an output flood (`yes`, big
                            // cat) would otherwise wake the UI per 8KB chunk;
                            // 8ms between wakes caps redraws at ~120Hz and
                            // applies gentle backpressure to the flood.
                            if last_wake.elapsed() >= std::time::Duration::from_millis(8) {
                                last_wake = std::time::Instant::now();
                                waker.wake();
                            }
                        }
                    }
                }
                waker.wake(); // final state after EOF
            });
        }

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            _slave: pair.slave,
            child,
            rows,
            cols,
            scrollback: 0,
            alive: true,
        })
    }

    /// Detect process exit. Call once per frame.
    pub fn pump(&mut self) {
        if self.alive
            && let Ok(Some(_)) = self.child.try_wait()
        {
            self.alive = false;
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Run a closure against the vt100 screen (the GPUI painter's read path).
    pub(crate) fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> Option<R> {
        self.parser.lock().ok().map(|p| f(p.screen()))
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Send input bytes (resets scrollback to live first, like the egui path).
    pub(crate) fn send_bytes(&mut self, bytes: &[u8]) {
        if self.scrollback != 0 {
            self.scrollback = 0;
            if let Ok(mut p) = self.parser.lock() {
                p.screen_mut().set_scrollback(0);
            }
        }
        self.write_bytes(bytes);
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Adjust scrollback by `delta` rows (clamped; positive = older).
    pub(crate) fn scroll_lines(&mut self, delta: i32) {
        let new = (self.scrollback as i32 + delta).clamp(0, SCROLLBACK as i32) as usize;
        if new != self.scrollback {
            self.scrollback = new;
            if let Ok(mut p) = self.parser.lock() {
                p.screen_mut().set_scrollback(self.scrollback);
            }
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    pub(crate) fn scrollback_pos(&self) -> usize {
        self.scrollback
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Public resize (rows/cols from the GPUI element's measured bounds).
    pub(crate) fn resize_to(&mut self, rows: u16, cols: u16) {
        self.resize(rows, cols);
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    pub(crate) fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// The visible screen text (for scanning dev-server URLs, etc.).
    pub fn screen_text(&self) -> String {
        self.parser
            .lock()
            .map(|p| p.screen().contents())
            .unwrap_or_default()
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if !self.alive || bytes.is_empty() {
            return;
        }
        match self.writer.lock() {
            Ok(mut w) => {
                if w.write_all(bytes).and_then(|()| w.flush()).is_err() {
                    self.alive = false;
                }
            }
            Err(_) => self.alive = false,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Map a key press (with modifiers) to a terminal input sequence, or `None` if
/// it's an ordinary printable char already delivered via `Event::Text`.
/// ONE key-encoding table for both shells, keyed by lowercase key names
/// (gpui keystroke names; the egui adapter maps `egui::Key` onto them).
pub(crate) fn named_key_seq(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        // GPUI delivers the spacebar as the NAMED key "space" (cf. the
        // "ctrl-space" keybinding syntax), and doesn't reliably populate
        // key_char for it -- so it must be handled here, not via the
        // printable fallthrough, or the spacebar does nothing in the PTY.
        "space" => b" ",
        "escape" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        _ => return None,
    })
}

/// Ctrl+<letter> -> control byte (Ctrl+A = 0x01 ... Ctrl+Z = 0x1a).
pub(crate) fn ctrl_byte(c: char) -> Option<u8> {
    c.is_ascii_lowercase().then_some((c as u8) & 0x1f)
}

/// The xterm 256-color cube + grays for indices 16..=255 (shared by both
/// shells; 0..=15 come from the active terminal theme's palette).
pub(crate) fn ansi_cube(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => (0, 0, 0), // caller resolves from the theme palette
        16..=231 => {
            let i = i - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (conv(r), conv(g), conv(b))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_key_table_and_ctrl_bytes() {
        // The ONE table the GPUI terminal encodes from.
        assert_eq!(named_key_seq("enter"), Some(b"\r" as &[u8]));
        assert_eq!(named_key_seq("backspace"), Some(b"\x7f" as &[u8]));
        assert_eq!(named_key_seq("up"), Some(b"\x1b[A" as &[u8]));
        assert_eq!(named_key_seq("pageup"), Some(b"\x1b[5~" as &[u8]));
        assert_eq!(named_key_seq("pagedown"), Some(b"\x1b[6~" as &[u8]));
        // GPUI hands the spacebar over as the named key "space"; it must
        // encode to a literal space, not fall through to a missing key_char.
        assert_eq!(named_key_seq("space"), Some(b" " as &[u8]));
        assert_eq!(named_key_seq("f5"), None);
        assert_eq!(ctrl_byte('c'), Some(0x03));
        assert_eq!(ctrl_byte('z'), Some(0x1a));
        assert_eq!(ctrl_byte('1'), None);
    }

    #[test]
    fn ansi_cube_matches_xterm() {
        assert_eq!(ansi_cube(16), (0, 0, 0)); // cube origin
        assert_eq!(ansi_cube(231), (255, 255, 255)); // cube max
        assert_eq!(ansi_cube(232), (8, 8, 8)); // grayscale start
    }
}
