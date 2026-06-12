//! The composer's real text input: a multiline adaptation of gpui's
//! `examples/input.rs` (EntityInputHandler — IME, cursor, selection, mouse).
//!
//! Differences from the example, documented for the next puppy:
//! - **Multiline, no soft wrap**: content may contain `\n`; each line is
//!   shaped separately and painted on its own row. Long lines clip (like a
//!   terminal) rather than wrap — soft wrap is editor-grade work we defer.
//! - **Events out**: the entity emits [`InputEvent`] (`Edited` on every
//!   change, `Submitted` on Enter); `shift-enter` inserts a newline.
//! - Key bindings live in `gpui_ui::run()` under the `"ChatInput"` context.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div,
    fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::tokens::Tokens;
use super::widgets::alpha;

actions!(
    chat_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Copy,
        Cut,
        ShowCharacterPalette,
        Submit,
        Newline,
    ]
);

/// Register the composer key bindings (call once at app startup).
pub fn bind_keys(cx: &mut App) {
    const CTX: Option<&str> = Some("ChatInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, CTX),
        KeyBinding::new("delete", Delete, CTX),
        KeyBinding::new("left", Left, CTX),
        KeyBinding::new("right", Right, CTX),
        KeyBinding::new("shift-left", SelectLeft, CTX),
        KeyBinding::new("shift-right", SelectRight, CTX),
        KeyBinding::new("cmd-a", SelectAll, CTX),
        KeyBinding::new("home", Home, CTX),
        KeyBinding::new("end", End, CTX),
        KeyBinding::new("cmd-v", Paste, CTX),
        KeyBinding::new("cmd-c", Copy, CTX),
        KeyBinding::new("cmd-x", Cut, CTX),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, CTX),
        KeyBinding::new("enter", Submit, CTX),
        KeyBinding::new("shift-enter", Newline, CTX),
    ]);
}

/// Events the composer surface listens for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// Content changed (typing, paste, IME commit, cut...).
    Edited,
    /// Enter pressed — send the prompt.
    Submitted,
}

/// Per-paint shaped layout (one entry per visual line).
struct LineLayout {
    lines: Vec<ShapedLine>,
    /// Byte offset where each line starts in `content`.
    starts: Vec<usize>,
    line_height: Pixels,
}

pub struct ChatInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<LineLayout>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    tokens: Tokens,
}

impl EventEmitter<InputEvent> for ChatInput {}

impl Focusable for ChatInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ChatInput {
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
            tokens: Tokens::dark(),
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.emit(InputEvent::Edited);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submitted);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
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
        // Start of the current line (multiline-aware Home).
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

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Multiline input: newlines survive the paste.
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
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
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let row = (f32::from(position.y - bounds.top()) / f32::from(layout.line_height)) as usize;
        let row = row.min(layout.lines.len().saturating_sub(1));
        let line_start = layout.starts[row];
        let col = layout.lines[row].closest_index_for_x(position.x - bounds.left());
        line_start + col
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
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
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
        let (row, col) = line_col(&layout.starts, range.start);
        let line = layout.lines.get(row)?;
        let y = bounds.top() + layout.line_height * row as f32;
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(col), y),
            point(
                bounds.left() + line.x_for_index(col),
                y + layout.line_height,
            ),
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

/// Map a global byte offset to (line index, byte column).
fn line_col(starts: &[usize], offset: usize) -> (usize, usize) {
    let row = starts.iter().rposition(|&s| s <= offset).unwrap_or(0);
    (row, offset - starts[row])
}

// ---------------------------------------------------------------------------
// The element
// ---------------------------------------------------------------------------

struct TextElement {
    input: Entity<ChatInput>,
}

struct PrepaintState {
    layout: Option<LineLayout>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
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
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_count = self.input.read(cx).content.split('\n').count().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        // Grow with content, capped at 8 rows (mock's composer height).
        style.size.height = (window.line_height() * line_count.min(8) as f32).into();
        (window.request_layout(style, [], cx), ())
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
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        let placeholder = content.is_empty();
        let text_color = if placeholder {
            gpui::Hsla::from(alpha(t.dim, 0.7))
        } else {
            style.color
        };

        // Shape each line separately; placeholder shapes as a single line.
        let source: Vec<&str> = if placeholder {
            vec![input.placeholder.as_ref()]
        } else {
            content.split('\n').collect()
        };
        let mut lines = Vec::with_capacity(source.len());
        let mut starts = Vec::with_capacity(source.len());
        let mut byte = 0usize;
        for seg in &source {
            starts.push(byte);
            let seg_string: SharedString = SharedString::from(seg.to_string());
            // IME underline: only applied within the line containing the
            // marked range (cross-line marked text is vanishingly rare).
            let runs = marked_runs(&style, text_color, &marked, byte, seg.len());
            lines.push(
                window
                    .text_system()
                    .shape_line(seg_string, font_size, &runs, None),
            );
            byte += seg.len() + 1; // +1 for the '\n'
        }
        let layout = LineLayout {
            lines,
            starts,
            line_height,
        };

        // Cursor + selection quads (skip while showing the placeholder).
        let mut selections = Vec::new();
        let mut cursor = None;
        if !placeholder {
            if selected_range.is_empty() {
                let (row, col) = line_col(&layout.starts, cursor_offset);
                if let Some(line) = layout.lines.get(row) {
                    let x = bounds.left() + line.x_for_index(col);
                    let y = bounds.top() + line_height * row as f32;
                    cursor = Some(fill(
                        Bounds::new(point(x, y), size(px(2.), line_height)),
                        t.accent,
                    ));
                }
            } else {
                let (sr, sc) = line_col(&layout.starts, selected_range.start);
                let (er, ec) = line_col(&layout.starts, selected_range.end);
                for row in sr..=er {
                    let Some(line) = layout.lines.get(row) else {
                        continue;
                    };
                    let x0 = if row == sr {
                        line.x_for_index(sc)
                    } else {
                        px(0.)
                    };
                    let x1 = if row == er {
                        line.x_for_index(ec)
                    } else {
                        line.width
                    };
                    let y = bounds.top() + line_height * row as f32;
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + x0, y),
                            point(bounds.left() + x1, y + line_height),
                        ),
                        alpha(t.accent, 0.25),
                    ));
                }
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
        for (row, line) in layout.lines.iter().enumerate() {
            let origin = point(
                bounds.origin.x,
                bounds.origin.y + layout.line_height * row as f32,
            );
            let _ = line.paint(origin, layout.line_height, window, cx);
        }
        if focus_handle.is_focused(window)
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

/// Text runs for one line, underlining any overlap with the IME marked range.
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_grow()
            .key_context("ChatInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::newline))
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
        // "ab\ncd\n e" → starts [0, 3, 6]
        let starts = vec![0usize, 3, 6];
        assert_eq!(line_col(&starts, 0), (0, 0));
        assert_eq!(line_col(&starts, 2), (0, 2));
        assert_eq!(line_col(&starts, 3), (1, 0));
        assert_eq!(line_col(&starts, 5), (1, 2));
        assert_eq!(line_col(&starts, 7), (2, 1));
    }
}
