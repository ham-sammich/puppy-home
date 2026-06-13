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

use eframe::egui;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::waker::UiWaker;

const SCROLLBACK: usize = 5000;
const FONT_SIZE: f32 = 13.0;

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
    focus_pending: bool,
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
            focus_pending: true,
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

    /// Render the terminal grid + handle keyboard/scroll. Fills `ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let font = egui::FontId::monospace(FONT_SIZE);
        let (cell_w, cell_h) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        if cell_w <= 0.0 || cell_h <= 0.0 {
            return;
        }

        let avail = ui.available_size();
        let cols = ((avail.x / cell_w).floor() as u16).max(1);
        let rows = ((avail.y / cell_h).floor() as u16).max(1);
        self.resize(rows, cols);

        // Reserve the area, then interact with a STABLE id so keyboard focus
        // persists across frames (auto-generated painter ids can drift).
        let (rect, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
        let term_id = ui.id().with("pty-grid");
        let resp = ui.interact(rect, term_id, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        if self.focus_pending {
            resp.request_focus();
            self.focus_pending = false;
        }
        if resp.clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            // Deliver Tab / arrows / Esc to the terminal instead of using them
            // for egui focus navigation.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    term_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }
        let origin = rect.min;

        // The active terminal palette is stashed in ctx data by the app layer.
        let term: crate::theme::ResolvedTerminal = ui
            .ctx()
            .data(|d| d.get_temp(crate::theme::terminal_colors_id()))
            .unwrap_or_default();
        let default_fg = term.fg;
        let default_bg = term.bg;

        // Mouse-wheel scrollback (when hovered and not on the alternate screen).
        if resp.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy.abs() > 0.5 {
                let delta = (dy / cell_h).round() as i32;
                let new = (self.scrollback as i32 + delta).clamp(0, SCROLLBACK as i32) as usize;
                if new != self.scrollback {
                    self.scrollback = new;
                    if let Ok(mut p) = self.parser.lock() {
                        p.screen_mut().set_scrollback(self.scrollback);
                    }
                }
            }
        }

        painter.rect_filled(resp.rect, 0.0, default_bg);

        if let Ok(parser) = self.parser.lock() {
            let screen = parser.screen();
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    let mut fg = to_color(cell.fgcolor(), default_fg, &term);
                    let mut bg = to_color(cell.bgcolor(), default_bg, &term);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    let x = origin.x + col as f32 * cell_w;
                    let y = origin.y + row as f32 * cell_h;
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
                    if bg != default_bg {
                        painter.rect_filled(rect, 0.0, bg);
                    }
                    let contents = cell.contents();
                    if !contents.is_empty() && contents != " " {
                        painter.text(rect.min, egui::Align2::LEFT_TOP, contents, font.clone(), fg);
                    }
                    if cell.underline() {
                        painter.hline(x..=x + cell_w, y + cell_h - 1.0, egui::Stroke::new(1.0, fg));
                    }
                }
            }

            // Cursor (only when live on the most recent screen).
            if self.scrollback == 0 && !screen.hide_cursor() {
                let (cr, cc) = screen.cursor_position();
                if cr < rows && cc < cols {
                    let x = origin.x + cc as f32 * cell_w;
                    let y = origin.y + cr as f32 * cell_h;
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
                    let cursor = term.cursor;
                    if focused {
                        painter.rect_filled(rect, 0.0, cursor.gamma_multiply(0.55));
                    } else {
                        painter.rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.0, cursor),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
        }

        if self.scrollback > 0 {
            painter.text(
                resp.rect.right_top() + egui::vec2(-6.0, 4.0),
                egui::Align2::RIGHT_TOP,
                format!("↑ {} (scroll to bottom to resume)", self.scrollback),
                egui::FontId::proportional(11.0),
                egui::Color32::from_rgb(0xd0, 0xc0, 0x60),
            );
        }

        if focused {
            painter.rect_stroke(
                resp.rect,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 90, 120)),
                egui::StrokeKind::Inside,
            );
            let bytes = self.collect_input(ui);
            if !bytes.is_empty() {
                if self.scrollback != 0 {
                    self.scrollback = 0;
                    if let Ok(mut p) = self.parser.lock() {
                        p.screen_mut().set_scrollback(0);
                    }
                }
                self.write_bytes(&bytes);
            }
        }
    }

    /// Translate this frame's key/text events into terminal bytes.
    fn collect_input(&self, ui: &mut egui::Ui) -> Vec<u8> {
        let mut out = Vec::new();
        ui.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Text(t) => out.extend_from_slice(t.as_bytes()),
                    egui::Event::Paste(t) => out.extend_from_slice(t.as_bytes()),
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if let Some(seq) = key_seq(*key, *modifiers) {
                            out.extend_from_slice(&seq);
                        }
                    }
                    _ => {}
                }
            }
        });
        out
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

fn key_seq(key: egui::Key, mods: egui::Modifiers) -> Option<Vec<u8>> {
    use egui::Key;
    let name = match key {
        Key::Enter => "enter",
        Key::Backspace => "backspace",
        Key::Tab => "tab",
        Key::Escape => "escape",
        Key::ArrowUp => "up",
        Key::ArrowDown => "down",
        Key::ArrowRight => "right",
        Key::ArrowLeft => "left",
        Key::Home => "home",
        Key::End => "end",
        Key::Delete => "delete",
        Key::Insert => "insert",
        Key::PageUp => "pageup",
        Key::PageDown => "pagedown",
        _ => {
            if mods.ctrl
                && let Some(c) = letter(key)
            {
                return ctrl_byte(c).map(|b| vec![b]);
            }
            return None;
        }
    };
    named_key_seq(name).map(<[u8]>::to_vec)
}

/// The lowercase letter for an A–Z key, else `None`.
fn letter(key: egui::Key) -> Option<char> {
    use egui::Key::*;
    Some(match key {
        A => 'a',
        B => 'b',
        C => 'c',
        D => 'd',
        E => 'e',
        F => 'f',
        G => 'g',
        H => 'h',
        I => 'i',
        J => 'j',
        K => 'k',
        L => 'l',
        M => 'm',
        N => 'n',
        O => 'o',
        P => 'p',
        Q => 'q',
        R => 'r',
        S => 's',
        T => 't',
        U => 'u',
        V => 'v',
        W => 'w',
        X => 'x',
        Y => 'y',
        Z => 'z',
        _ => return None,
    })
}

/// Convert a vt100 color to an egui color, honoring the active terminal theme
/// (its 16 base ANSI colors) and the 256-color cube for higher indices.
fn to_color(
    c: vt100::Color,
    default: egui::Color32,
    term: &crate::theme::ResolvedTerminal,
) -> egui::Color32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
        vt100::Color::Idx(i) => ansi_256(i, term),
    }
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

fn ansi_256(i: u8, term: &crate::theme::ResolvedTerminal) -> egui::Color32 {
    match i {
        0..=15 => term.ansi[i as usize],
        _ => {
            let (r, g, b) = ansi_cube(i);
            egui::Color32::from_rgb(r, g, b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn key_sequences() {
        assert_eq!(
            key_seq(Key::Enter, Modifiers::NONE).as_deref(),
            Some(&b"\r"[..])
        );
        assert_eq!(
            key_seq(Key::Backspace, Modifiers::NONE).as_deref(),
            Some(&b"\x7f"[..])
        );
        assert_eq!(
            key_seq(Key::ArrowUp, Modifiers::NONE).as_deref(),
            Some(&b"\x1b[A"[..])
        );
        assert_eq!(
            key_seq(Key::ArrowLeft, Modifiers::NONE).as_deref(),
            Some(&b"\x1b[D"[..])
        );
        assert_eq!(
            key_seq(Key::Home, Modifiers::NONE).as_deref(),
            Some(&b"\x1b[H"[..])
        );
        // Ctrl+C → 0x03 (SIGINT), Ctrl+D → 0x04.
        assert_eq!(key_seq(Key::C, ctrl()).as_deref(), Some(&[0x03u8][..]));
        assert_eq!(key_seq(Key::D, ctrl()).as_deref(), Some(&[0x04u8][..]));
        // Plain letters are delivered via Event::Text, not key_seq.
        assert_eq!(key_seq(Key::A, Modifiers::NONE), None);
    }

    #[test]
    fn shared_key_table_and_ctrl_bytes() {
        // The ONE table both shells encode from.
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
    fn ansi_palette() {
        let term = crate::theme::ResolvedTerminal::default();
        assert_eq!(ansi_256(1, &term), egui::Color32::from_rgb(205, 49, 49)); // red
        assert_eq!(ansi_256(16, &term), egui::Color32::from_rgb(0, 0, 0)); // cube origin
        assert_eq!(ansi_256(231, &term), egui::Color32::from_rgb(255, 255, 255)); // cube max
        assert_eq!(ansi_256(232, &term), egui::Color32::from_rgb(8, 8, 8)); // grayscale start
    }

    #[test]
    fn color_conversion() {
        let term = crate::theme::ResolvedTerminal::default();
        let def = egui::Color32::from_rgb(1, 2, 3);
        assert_eq!(to_color(vt100::Color::Default, def, &term), def);
        assert_eq!(
            to_color(vt100::Color::Rgb(10, 20, 30), def, &term),
            egui::Color32::from_rgb(10, 20, 30)
        );
        assert_eq!(
            to_color(vt100::Color::Idx(1), def, &term),
            egui::Color32::from_rgb(205, 49, 49)
        );
    }
}
