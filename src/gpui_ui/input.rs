//! The composer's text input: a soft-wrapping multiline adaptation of gpui's
//! `examples/input.rs` (EntityInputHandler — IME, cursor, selection, mouse).
//!
//! B1c upgrades over the 2.3 version, for the next puppy:
//! - **Soft wrap**: logical lines are shaped with `shape_text(wrap_width)`
//!   into [`WrappedLine`]s; cursor/selection/mouse geometry is multi-row
//!   aware via `position_for_index` / `index_for_position`. Height comes
//!   from a measured layout (`request_measured_layout`), capped at
//!   [`MAX_VISIBLE_ROWS`].
//! - **Up/Down** move across visual rows; at the top/bottom edge they emit
//!   `HistoryPrev`/`HistoryNext` (shell-style prompt recall, root-handled).
//! - **Word jump**: alt/ctrl+arrows (+shift to select).
//! - **Palette routing**: while `palette_open` is set by the root,
//!   Up/Down/Enter/Tab/Escape become palette events instead of edits.
//! - **Image paste**: clipboard image entries emit `InputEvent::Image`
//!   (PNG bytes); non-PNG clipboards fall back to the shared arboard
//!   pipeline (`workspace::clipboard`).
//!
//! Deliberate punts: no goal-column stickiness on Up/Down, no internal
//! scroll past [`MAX_VISIBLE_ROWS`] (content clips), no cursor blink.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardEntry, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, ImageFormat,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, SharedString, Style, Subscription, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};

/// Caret blink half-period: solid this long, then hidden this long (~VSCode).
const CARET_BLINK: Duration = Duration::from_millis(530);
use unicode_segmentation::UnicodeSegmentation;

use super::tokens::Tokens;
use super::widgets::alpha;

/// Height cap, in visual rows (the mock's composer height).
const MAX_VISIBLE_ROWS: usize = 8;

actions!(
    chat_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Paste,
        Copy,
        Cut,
        ShowCharacterPalette,
        Submit,
        Newline,
        EscapeKey,
        TabKey,
        SaveFile,
        Undo,
        Redo,
    ]
);

/// Max retained undo snapshots (full-content copies; bounded so a long
/// editing session can't grow memory without limit).
const UNDO_CAP: usize = 256;

/// A point-in-time editor state for undo/redo. Captures content + caret so
/// undo restores both the text and where you were.
#[derive(Clone)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

/// Register the composer key bindings (call once at app startup).
pub fn bind_keys(cx: &mut App) {
    const CTX: Option<&str> = Some("ChatInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, CTX),
        KeyBinding::new("delete", Delete, CTX),
        KeyBinding::new("left", Left, CTX),
        KeyBinding::new("right", Right, CTX),
        KeyBinding::new("up", Up, CTX),
        KeyBinding::new("down", Down, CTX),
        KeyBinding::new("shift-left", SelectLeft, CTX),
        KeyBinding::new("shift-right", SelectRight, CTX),
        KeyBinding::new("cmd-a", SelectAll, CTX),
        KeyBinding::new("ctrl-a", SelectAll, CTX),
        KeyBinding::new("home", Home, CTX),
        KeyBinding::new("end", End, CTX),
        KeyBinding::new("cmd-left", Home, CTX),
        KeyBinding::new("cmd-right", End, CTX),
        // Word jump: alt on macOS, ctrl elsewhere — bind both, they don't clash.
        KeyBinding::new("alt-left", WordLeft, CTX),
        KeyBinding::new("alt-right", WordRight, CTX),
        KeyBinding::new("ctrl-left", WordLeft, CTX),
        KeyBinding::new("ctrl-right", WordRight, CTX),
        KeyBinding::new("alt-shift-left", SelectWordLeft, CTX),
        KeyBinding::new("alt-shift-right", SelectWordRight, CTX),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, CTX),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, CTX),
        KeyBinding::new("cmd-v", Paste, CTX),
        KeyBinding::new("cmd-c", Copy, CTX),
        KeyBinding::new("cmd-x", Cut, CTX),
        // Windows/Linux use ctrl for clipboard + select-all; bind both so the
        // editor + composer copy/paste/cut work off-macOS (G3 gate finding —
        // only cmd-* was bound, so Ctrl+C/V/X/A silently did nothing).
        KeyBinding::new("ctrl-v", Paste, CTX),
        KeyBinding::new("ctrl-c", Copy, CTX),
        KeyBinding::new("ctrl-x", Cut, CTX),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, CTX),
        KeyBinding::new("enter", Submit, CTX),
        KeyBinding::new("shift-enter", Newline, CTX),
        KeyBinding::new("escape", EscapeKey, CTX),
        KeyBinding::new("tab", TabKey, CTX),
        KeyBinding::new("cmd-s", SaveFile, CTX),
        KeyBinding::new("ctrl-s", SaveFile, CTX),
        // Undo/redo: cmd on macOS, ctrl elsewhere. ctrl-y is the common
        // Windows redo; cmd/ctrl-shift-z covers the rest.
        KeyBinding::new("cmd-z", Undo, CTX),
        KeyBinding::new("ctrl-z", Undo, CTX),
        KeyBinding::new("cmd-shift-z", Redo, CTX),
        KeyBinding::new("ctrl-shift-z", Redo, CTX),
        KeyBinding::new("ctrl-y", Redo, CTX),
    ]);
}

/// Events the composer surface listens for.
#[derive(Clone, Debug)]
pub enum InputEvent {
    /// Content changed (typing, paste, IME commit, cut...).
    Edited,
    /// Enter pressed (palette closed) — send the prompt.
    Submitted,
    /// Up pressed on the first visual row — recall older prompt.
    HistoryPrev,
    /// Down pressed on the last visual row — recall newer prompt / draft.
    HistoryNext,
    /// Up/Down while the completion palette is open.
    PaletteNav(i32),
    /// Enter/Tab while the palette is open.
    PaletteAccept,
    /// Escape while the palette is open.
    PaletteDismiss,
    /// An image was pasted (PNG bytes).
    Image(Vec<u8>),
    /// Cmd/Ctrl+S (editor surfaces route this to a file save).
    Save,
}

/// Per-logical-line syntax runs: `(byte_len, color)` segments covering the
/// line (the editor recomputes these on change, never per frame).
pub type SyntaxRuns = std::sync::Arc<Vec<Vec<(usize, gpui::Hsla)>>>;

/// Per-paint wrapped layout: one [`WrappedLine`] per logical line.
struct WrapLayout {
    lines: Vec<WrappedLine>,
    /// Byte offset where each logical line starts in `content`.
    starts: Vec<usize>,
    /// Y offset of each logical line's first visual row.
    y_offsets: Vec<Pixels>,
    line_height: Pixels,
    total_height: Pixels,
}

impl WrapLayout {
    /// Top-left of the caret for a global byte offset.
    fn pos_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let (row, col) = line_col(&self.starts, offset);
        let line = self.lines.get(row)?;
        let base = self.y_offsets[row];
        match line.position_for_index(col, self.line_height) {
            Some(p) => Some(point(p.x, p.y + base)),
            // End-of-text on an empty trailing line (or any miss): line start.
            None => Some(point(px(0.), base)),
        }
    }

    /// Closest byte offset for a point relative to the element origin.
    fn offset_for_point(&self, p: Point<Pixels>, content_len: usize) -> usize {
        if p.y < px(0.) {
            return 0;
        }
        for (i, line) in self.lines.iter().enumerate() {
            let rows = line.wrap_boundaries().len() + 1;
            let h = self.line_height * rows as f32;
            if p.y < self.y_offsets[i] + h {
                let local = point(p.x.max(px(0.)), (p.y - self.y_offsets[i]).max(px(0.)));
                let ix = match line.index_for_position(local, self.line_height) {
                    Ok(ix) | Err(ix) => ix,
                };
                return self.starts[i] + ix.min(line.text.len());
            }
        }
        content_len
    }
}

pub struct ChatInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<std::sync::Arc<WrapLayout>>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    tokens: Tokens,
    /// Set by the root while the completion palette is showing: nav keys
    /// route to the palette instead of the buffer.
    pub palette_open: bool,
    /// Code mode: no soft wrap (horizontal scroll), no visible-row cap.
    soft_wrap: bool,
    /// Syntax color runs (code mode); see [`SyntaxRuns`].
    syntax: Option<SyntaxRuns>,
    /// Bumped on every content/syntax change — keys the layout cache.
    generation: u64,
    /// Cached shaped layout, keyed by (generation, wrap-width px). Editors
    /// re-render on every drain notify; reshaping a whole file per frame
    /// would be O(file) at 4Hz, so we shape only when content/width change.
    cache: std::cell::RefCell<Option<(u64, i32, std::sync::Arc<WrapLayout>)>>,
    // -- caret blink (only ticks while focused; see `start_blink`) --
    /// Phase reference; reset on focus and on every edit/move so the caret
    /// stays SOLID while you're actively typing, then blinks when idle.
    blink_epoch: Instant,
    /// Whether this input currently holds focus (driven by focus listeners).
    blink_focused: bool,
    /// Guard so only one blink ticker task runs at a time.
    blinking: bool,
    /// Focus/blur subscriptions, registered lazily at first render.
    blink_subs: Vec<Subscription>,
    // -- undo/redo --
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    /// True while a run of single-character typing is being coalesced into
    /// one undo step (so undo reverts a whole word, not one keystroke).
    typing_run: bool,
}

impl EventEmitter<InputEvent> for ChatInput {}

impl Focusable for ChatInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ChatInput {
    /// Re-resolve this input's colors after a theme switch (the root owns
    /// the active palette; entities can't read it back, so it's pushed).
    pub fn set_tokens(&mut self, t: Tokens, cx: &mut Context<Self>) {
        self.tokens = t;
        // The shaped-run color comes from these tokens (B13.2 redux) and
        // the cache key is only (generation, wrap) — drop it or a theme
        // switch keeps the old-palette colors until the next edit.
        *self.cache.borrow_mut() = None;
        cx.notify();
    }

    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        ChatInput {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            tokens: Tokens::current(),
            palette_open: false,
            soft_wrap: true,
            syntax: None,
            generation: 0,
            cache: std::cell::RefCell::new(None),
            blink_epoch: Instant::now(),
            blink_focused: false,
            blinking: false,
            blink_subs: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            typing_run: false,
        }
    }

    /// Reset the blink phase — keeps the caret solid through edits and
    /// cursor moves (call wherever content or the selection changes).
    fn touch_caret(&mut self) {
        self.blink_epoch = Instant::now();
    }

    /// Is the caret in its visible half right now? Solid for the first
    /// half-period after `blink_epoch`, hidden the next, repeating.
    fn caret_on(&self) -> bool {
        let half = CARET_BLINK.as_millis().max(1);
        (self.blink_epoch.elapsed().as_millis() / half).is_multiple_of(2)
    }

    /// Register focus/blur listeners once (needs a `Window`, so called from
    /// `render`). Also catches the case where the input is already focused.
    fn ensure_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.blink_subs.is_empty() {
            return;
        }
        let handle = self.focus_handle.clone();
        let on_focus = cx.on_focus(&handle, window, |this, _, cx| {
            this.blink_focused = true;
            this.touch_caret();
            this.start_blink(cx);
            cx.notify();
        });
        let on_blur = cx.on_blur(&handle, window, |this, _, cx| {
            this.blink_focused = false;
            cx.notify();
        });
        self.blink_subs = vec![on_focus, on_blur];
        if self.focus_handle.is_focused(window) {
            self.blink_focused = true;
            self.touch_caret();
            self.start_blink(cx);
        }
    }

    /// Coarse blink ticker: wakes ~2x/sec to repaint, and exits the moment
    /// focus is lost — no idle vsync spin (G1 audit discipline).
    fn start_blink(&mut self, cx: &mut Context<Self>) {
        if self.blinking {
            return;
        }
        self.blinking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let keep = this.update(cx, |this, cx| {
                    if this.blink_focused {
                        cx.notify();
                    }
                    this.blink_focused
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.blinking = false);
        })
        .detach();
    }

    /// Code-editor mode: no soft wrap, no row cap, syntax runs supported.
    pub fn new_code(cx: &mut Context<Self>) -> Self {
        let mut this = Self::new("", cx);
        this.soft_wrap = false;
        this
    }

    /// Install/replace syntax runs (recomputed by the editor on change).
    pub fn set_syntax(&mut self, runs: Option<SyntaxRuns>, cx: &mut Context<Self>) {
        self.syntax = runs;
        self.generation += 1;
        cx.notify();
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        // Programmatic load (e.g. opening a file) is not an undoable edit:
        // start the history fresh so undo can't reach the previous buffer.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.typing_run = false;
        self.generation += 1;
        self.touch_caret();
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            cx.emit(InputEvent::PaletteAccept);
        } else if !self.soft_wrap {
            // Code mode: Enter is a newline, not a send.
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(InputEvent::Submitted);
        }
    }

    fn save_file(&mut self, _: &SaveFile, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Save);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn escape(&mut self, _: &EscapeKey, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            cx.emit(InputEvent::PaletteDismiss);
        }
    }

    fn tab(&mut self, _: &TabKey, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            cx.emit(InputEvent::PaletteAccept);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            cx.emit(InputEvent::PaletteNav(-1));
            return;
        }
        let cur = self.cursor_offset();
        let Some((layout, p)) = self
            .last_layout
            .as_ref()
            .and_then(|l| l.pos_for_offset(cur).map(|p| (l, p)))
        else {
            cx.emit(InputEvent::HistoryPrev);
            return;
        };
        if p.y < layout.line_height {
            cx.emit(InputEvent::HistoryPrev); // top edge -> history
            return;
        }
        let target = point(p.x, p.y - layout.line_height / 2.);
        let offset = layout.offset_for_point(target, self.content.len());
        self.move_to(offset, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            cx.emit(InputEvent::PaletteNav(1));
            return;
        }
        let cur = self.cursor_offset();
        let Some((layout, p)) = self
            .last_layout
            .as_ref()
            .and_then(|l| l.pos_for_offset(cur).map(|p| (l, p)))
        else {
            cx.emit(InputEvent::HistoryNext);
            return;
        };
        if p.y + layout.line_height * 1.5 > layout.total_height {
            cx.emit(InputEvent::HistoryNext); // bottom edge -> history
            return;
        }
        let target = point(p.x, p.y + layout.line_height * 1.5);
        let offset = layout.offset_for_point(target, self.content.len());
        self.move_to(offset, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cur = self.cursor_offset();
        let start = self.content[..cur].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let cur = self.cursor_offset();
        let end = self.content[cur..]
            .find('\n')
            .map_or(self.content.len(), |i| cur + i);
        self.move_to(end, cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = prev_word_boundary(&self.content, self.cursor_offset());
        self.move_to(target, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = next_word_boundary(&self.content, self.cursor_offset());
        self.move_to(target, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = prev_word_boundary(&self.content, self.cursor_offset());
        self.select_to(target, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = next_word_boundary(&self.content, self.cursor_offset());
        self.select_to(target, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if ev.modifiers.shift {
            self.select_to(self.index_for_mouse_position(ev.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(ev.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(ev.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    /// Paste text and/or an image. gpui clipboard image entries are used
    /// when present (PNG passes straight through); otherwise the shared
    /// arboard pipeline converts whatever the OS has into PNG bytes.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = String::new();
        let mut image_png: Option<Vec<u8>> = None;
        if let Some(item) = cx.read_from_clipboard() {
            for entry in item.into_entries() {
                match entry {
                    ClipboardEntry::String(s) => text.push_str(s.text()),
                    ClipboardEntry::Image(img) if image_png.is_none() => {
                        if img.format == ImageFormat::Png {
                            image_png = Some(img.bytes);
                        }
                    }
                    ClipboardEntry::Image(_) => {}
                    ClipboardEntry::ExternalPaths(_) => {}
                }
            }
        }
        if image_png.is_none() && text.is_empty() {
            // Non-PNG or platform path: shared arboard RGBA -> PNG pipeline.
            image_png = crate::workspace::clipboard::read_clipboard_image().and_then(|img| {
                crate::workspace::clipboard::encode_png(img.width, img.height, &img.rgba)
            });
        }
        if let Some(png) = image_png {
            cx.emit(InputEvent::Image(png));
        }
        if !text.is_empty() {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    /// Snapshot the CURRENT buffer state onto the undo stack and invalidate
    /// the redo stack (a fresh edit forks history). Bounded by `UNDO_CAP`.
    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_CAP {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn apply_snapshot(&mut self, s: EditSnapshot, cx: &mut Context<Self>) {
        self.content = s.content;
        let len = self.content.len();
        // Defensive clamp: a snapshot's range is valid for its own content,
        // but clamp anyway so no future change can ever panic-slice.
        self.selected_range = s.selected_range.start.min(len)..s.selected_range.end.min(len);
        self.selection_reversed = s.selection_reversed;
        self.marked_range = None;
        self.typing_run = false;
        self.generation += 1;
        self.touch_caret();
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.undo_stack.pop() {
            let cur = self.snapshot();
            self.redo_stack.push(cur);
            self.apply_snapshot(prev, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.redo_stack.pop() {
            let cur = self.snapshot();
            self.undo_stack.push(cur);
            self.apply_snapshot(next, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.typing_run = false; // a cursor move ends the current typing run
        self.touch_caret();
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(layout)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        layout.offset_for_point(
            point(position.x - bounds.left(), position.y - bounds.top()),
            self.content.len(),
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.typing_run = false; // selecting ends the current typing run
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

/// Previous word start strictly before `offset` (whitespace runs skipped).
fn prev_word_boundary(content: &str, offset: usize) -> usize {
    let mut target = 0;
    for (i, w) in content.split_word_bound_indices() {
        if i >= offset {
            break;
        }
        if !w.trim().is_empty() {
            target = i;
        }
    }
    target
}

/// End of the next word at/after `offset` (whitespace runs skipped).
fn next_word_boundary(content: &str, offset: usize) -> usize {
    for (i, w) in content.split_word_bound_indices() {
        let end = i + w.len();
        if end > offset && !w.trim().is_empty() {
            return end;
        }
    }
    content.len()
}

/// Map a global byte offset to (logical line index, byte column).
fn line_col(starts: &[usize], offset: usize) -> (usize, usize) {
    let row = starts.iter().rposition(|&s| s <= offset).unwrap_or(0);
    (row, offset - starts[row])
}

impl ChatInput {
    /// The shaped layout for this content at `wrap_px` (-1 = no wrap),
    /// served from the generation-keyed cache; shaping happens only when
    /// content/syntax/width actually changed.
    fn cached_layout(&self, wrap_px: i32, window: &mut Window) -> std::sync::Arc<WrapLayout> {
        if let Some((generation, key, layout)) = self.cache.borrow().as_ref()
            && *generation == self.generation
            && *key == wrap_px
        {
            return layout.clone();
        }
        // Plain-text runs are colored from the input's OWN tokens, not the
        // ambient div cascade: shaped runs don't inherit `text_color` from
        // ancestors, and `window.text_style()` falls back to gpui's default
        // (black) whenever a container forgot to set one — which is exactly
        // how every plain input went black-on-dark (B13.2 redux). The
        // tokens are pushed by the root on theme switches (`set_tokens`).
        let color = gpui::Hsla::from(self.tokens.text);
        let wrap = (wrap_px >= 0).then(|| px(wrap_px as f32));
        let layout = std::sync::Arc::new(shape_lines(
            &self.content,
            &self.marked_range,
            self.syntax.as_ref(),
            color,
            wrap,
            window,
        ));
        *self.cache.borrow_mut() = Some((self.generation, wrap_px, layout.clone()));
        layout
    }
}

impl EntityInputHandler for ChatInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // Undo bookkeeping: snapshot the pre-edit state, coalescing a run of
        // single-char insertions into one step. Replacements, deletions,
        // pastes and newlines each start a fresh undo group.
        let is_typing =
            range.start == range.end && new_text != "\n" && new_text.chars().count() == 1;
        if !(is_typing && self.typing_run) {
            self.push_undo();
        }
        self.typing_run = is_typing;
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.generation += 1;
        self.touch_caret();
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.generation += 1;
        self.touch_caret();
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let p = layout.pos_for_offset(range.start)?;
        Some(Bounds::from_corners(
            point(bounds.left() + p.x, bounds.top() + p.y),
            point(bounds.left() + p.x, bounds.top() + p.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let utf8 = self.index_for_mouse_position(point);
        Some(self.offset_to_utf16(utf8))
    }
}

// ---------------------------------------------------------------------------
// The element
// ---------------------------------------------------------------------------

struct TextElement {
    input: Entity<ChatInput>,
}

struct PrepaintState {
    layout: Option<std::sync::Arc<WrapLayout>>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

/// Shape `content` into logical lines; `wrap_width: None` = code mode (no
/// soft wrap, one visual row per logical line, horizontal overflow).
fn shape_lines(
    content: &str,
    marked: &Option<Range<usize>>,
    syntax: Option<&SyntaxRuns>,
    color: gpui::Hsla,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> WrapLayout {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = window.line_height();
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut y_offsets = Vec::new();
    let mut y = px(0.);
    let mut byte = 0usize;
    for (line_ix, seg) in content.split('\n').enumerate() {
        starts.push(byte);
        y_offsets.push(y);
        let runs = match syntax.and_then(|sy| sy.get(line_ix)) {
            Some(spans) => syntax_runs(&style, color, spans, seg.len()),
            None => marked_runs(&style, color, marked, byte, seg.len()),
        };
        let shaped = window
            .text_system()
            .shape_text(
                SharedString::from(seg.to_string()),
                font_size,
                &runs,
                wrap_width,
                None,
            )
            .ok()
            .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
            .unwrap_or_default();
        let rows = shaped.wrap_boundaries().len() + 1;
        y += line_height * rows as f32;
        lines.push(shaped);
        byte += seg.len() + 1;
    }
    WrapLayout {
        lines,
        starts,
        y_offsets,
        line_height,
        total_height: y,
    }
}

impl gpui::Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.clone();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let line_height = window.line_height();
                let avail = known.width.unwrap_or(match available.width {
                    gpui::AvailableSpace::Definite(w) => w,
                    _ => px(360.),
                });
                let state = input.read(cx);
                if state.content.is_empty() {
                    return size(avail, line_height);
                }
                if state.soft_wrap {
                    let layout = state.cached_layout(f32::from(avail) as i32, window);
                    let max = line_height * MAX_VISIBLE_ROWS as f32;
                    size(avail, layout.total_height.min(max).max(line_height))
                } else {
                    // Code mode: width = widest line (horizontal scroll),
                    // height = every row (vertical scroll is the parent's).
                    let layout = state.cached_layout(-1, window);
                    let widest = layout
                        .lines
                        .iter()
                        .map(|l| f32::from(l.width()))
                        .fold(0.0f32, f32::max);
                    size(
                        px(widest + 8.0).max(avail),
                        layout.total_height.max(line_height),
                    )
                }
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let marked = input.marked_range.clone();
        let t = input.tokens;
        let style = window.text_style();

        let placeholder = content.is_empty();
        let layout = if placeholder {
            std::sync::Arc::new(shape_lines(
                input.placeholder.as_ref(),
                &None,
                None,
                gpui::Hsla::from(alpha(t.dim, 0.7)),
                Some(bounds.size.width),
                window,
            ))
        } else {
            let key = if input.soft_wrap {
                f32::from(bounds.size.width) as i32
            } else {
                -1
            };
            input.cached_layout(key, window)
        };
        let _ = (style, marked);

        let mut selections = Vec::new();
        let mut cursor = None;
        // The caret is drawn whenever there's no selection — INCLUDING an
        // empty input (placeholder shown). Otherwise a freshly-focused empty
        // box gives no "you are here" signal until the first keystroke.
        if selected_range.is_empty() {
            // Empty content: anchor the caret at the input's start instead of
            // the (dimmed) placeholder glyph positions.
            let p = if placeholder {
                point(px(0.), px(0.))
            } else {
                layout
                    .pos_for_offset(cursor_offset)
                    .unwrap_or(point(px(0.), px(0.)))
            };
            cursor = Some(fill(
                Bounds::new(
                    point(bounds.left() + p.x, bounds.top() + p.y),
                    size(px(2.), layout.line_height),
                ),
                t.accent,
            ));
        } else if !placeholder
            && let (Some(p1), Some(p2)) = (
                layout.pos_for_offset(selected_range.start),
                layout.pos_for_offset(selected_range.end),
            )
        {
            // One quad per visual row the selection touches.
            let lh = layout.line_height;
            let sel = alpha(t.accent, 0.25);
            let mut y = p1.y;
            while y <= p2.y {
                let x0 = if y == p1.y { p1.x } else { px(0.) };
                let x1 = if y == p2.y { p2.x } else { bounds.size.width };
                if x1 > x0 {
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + x0, bounds.top() + y),
                            point(bounds.left() + x1, bounds.top() + y + lh),
                        ),
                        sel,
                    ));
                }
                y += lh;
            }
        }
        PrepaintState {
            layout: Some(layout),
            cursor,
            selections,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for sel in prepaint.selections.drain(..) {
            window.paint_quad(sel);
        }
        let layout = prepaint.layout.take().unwrap();
        for (i, line) in layout.lines.iter().enumerate() {
            let origin = point(bounds.origin.x, bounds.origin.y + layout.y_offsets[i]);
            let _ = line.paint(
                origin,
                layout.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
        // Caret paints only while focused AND in the visible half of the
        // blink cycle (a freshly-focused or actively-typing caret is solid).
        if focus_handle.is_focused(window)
            && self.input.read(cx).caret_on()
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(layout);
            input.last_bounds = Some(bounds);
        });
    }
}

/// Syntax-colored text runs for one logical line (code mode); the run list
/// is clamped/padded so lengths always sum to the line length. IME marked
/// underline is skipped in code mode (documented punt).
fn syntax_runs(
    style: &gpui::TextStyle,
    fallback: gpui::Hsla,
    spans: &[(usize, gpui::Hsla)],
    line_len: usize,
) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(spans.len() + 1);
    let mut covered = 0usize;
    for (len, color) in spans {
        if covered >= line_len {
            break;
        }
        let len = (*len).min(line_len - covered);
        if len == 0 {
            continue;
        }
        out.push(TextRun {
            len,
            font: style.font(),
            color: *color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        covered += len;
    }
    if covered < line_len {
        out.push(TextRun {
            len: line_len - covered,
            font: style.font(),
            color: fallback,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    if out.is_empty() {
        out.push(TextRun {
            len: line_len,
            font: style.font(),
            color: fallback,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    out
}

/// Text runs for one logical line, underlining any IME marked-range overlap.
fn marked_runs(
    style: &gpui::TextStyle,
    color: gpui::Hsla,
    marked: &Option<Range<usize>>,
    line_start: usize,
    line_len: usize,
) -> Vec<TextRun> {
    let base = TextRun {
        len: line_len,
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let Some(m) = marked else {
        return vec![base];
    };
    let line_end = line_start + line_len;
    let s = m.start.clamp(line_start, line_end) - line_start;
    let e = m.end.clamp(line_start, line_end) - line_start;
    if s >= e {
        return vec![base];
    }
    [
        TextRun {
            len: s,
            ..base.clone()
        },
        TextRun {
            len: e - s,
            underline: Some(UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            }),
            ..base.clone()
        },
        TextRun {
            len: line_len - e,
            ..base.clone()
        },
    ]
    .into_iter()
    .filter(|run| run.len > 0)
    .collect()
}

impl Render for ChatInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_blink(window, cx);
        div()
            .flex_grow_1()
            .key_context("ChatInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|this, _: &Cut, window, cx| this.cut(window, cx)))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::save_file))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::tab))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .text_size(px(13.))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_maps_offsets() {
        let starts = vec![0usize, 3, 6];
        assert_eq!(line_col(&starts, 0), (0, 0));
        assert_eq!(line_col(&starts, 2), (0, 2));
        assert_eq!(line_col(&starts, 3), (1, 0));
        assert_eq!(line_col(&starts, 5), (1, 2));
        assert_eq!(line_col(&starts, 7), (2, 1));
    }

    #[test]
    fn word_boundaries_skip_whitespace() {
        let s = "run cargo  test now";
        assert_eq!(prev_word_boundary(s, 9), 4); // mid-"cargo" -> its start
        assert_eq!(prev_word_boundary(s, 4), 0); // at "cargo" start -> "run"
        assert_eq!(prev_word_boundary(s, 0), 0);
        assert_eq!(next_word_boundary(s, 0), 3); // end of "run"
        assert_eq!(next_word_boundary(s, 3), 9); // end of "cargo"
        assert_eq!(next_word_boundary(s, 18), 19);
        assert_eq!(next_word_boundary(s, 19), 19);
    }
}
