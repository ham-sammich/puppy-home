//! The embedded terminal as a GPUI surface: snapshot the vt100 screen each
//! render (built only when a render actually happens — PTY output wakes the
//! drain loop, which notifies), paint it in a canvas as per-row shaped lines
//! with coalesced color runs, route keys/paste/scroll to the PTY.
//!
//! Feature set matches the egui grid EXACTLY (no inventions): fg/bg/
//! inverse/underline cell attributes (egui ignores bold/italic — so do we),
//! block cursor (filled when focused, outlined when not), wheel scrollback
//! with the "scroll to bottom" banner, the shared key-encoding table +
//! Ctrl-chords, paste. No mouse selection-copy and no mouse reporting —
//! egui has neither.

use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, Bounds, Entity, FocusHandle, IntoElement, KeyDownEvent, ParentElement as _, Pixels,
    Rgba, ScrollWheelEvent, SharedString, Styled as _, TextRun, div, fill, point, prelude::*, px,
    size,
};

use crate::gpui_ui::widgets::alpha;
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::terminal::{ansi_cube, ctrl_byte, named_key_seq};
use crate::workspace::{Workspace, WorkspaceId};

/// Desired grid size discovered during paint, applied by the root next
/// render (elements can't mutate entities mid-paint).
pub type ResizeSlot = Arc<Mutex<Option<(WorkspaceId, u16, u16)>>>;

/// Terminal palette resolved to gpui colors (from the shared terminal.json
/// theme — same file the egui shell edits).
#[derive(Clone)]
pub struct TermColors {
    pub fg: Rgba,
    pub bg: Rgba,
    pub cursor: Rgba,
    pub ansi: [Rgba; 16],
}

impl TermColors {
    pub fn load() -> Self {
        Self::from_theme(&crate::theme::load_terminal())
    }

    /// Resolve from an in-memory terminal theme (the editor's live apply).
    pub fn from_theme(theme: &crate::theme::TerminalTheme) -> Self {
        let hex = crate::gpui_ui::tokens::hex;
        let mut ansi = [hex("#000000"); 16];
        for (i, slot) in ansi.iter_mut().enumerate() {
            if let Some(h) = theme.ansi.get(i) {
                *slot = hex(h);
            }
        }
        TermColors {
            fg: hex(&theme.fg),
            bg: hex(&theme.bg),
            cursor: hex(&theme.cursor),
            ansi,
        }
    }

    fn vt_color(&self, c: vt100::Color, default: Rgba) -> Rgba {
        match c {
            vt100::Color::Default => default,
            vt100::Color::Rgb(r, g, b) => Rgba {
                r: r as f32 / 255.0,
                g: g as f32 / 255.0,
                b: b as f32 / 255.0,
                a: 1.0,
            },
            vt100::Color::Idx(i) if i < 16 => self.ansi[i as usize],
            vt100::Color::Idx(i) => {
                let (r, g, b) = ansi_cube(i);
                Rgba {
                    r: r as f32 / 255.0,
                    g: g as f32 / 255.0,
                    b: b as f32 / 255.0,
                    a: 1.0,
                }
            }
        }
    }
}

/// One run of visually-identical cells in a row.
struct Run {
    len: usize, // byte length within the row text
    fg: Rgba,
    bg: Option<Rgba>,
    underline: bool,
}

struct RowSnap {
    text: String,
    runs: Vec<Run>,
}

struct TermSnap {
    rows: Vec<RowSnap>,
    cursor: Option<(u16, u16)>,
    scrollback: usize,
    alive: bool,
    grid: (u16, u16),
}

/// Build the paint snapshot from the live vt100 screen (cells coalesced by
/// attribute; inverse resolved here, exactly like the egui painter).
fn snapshot(ws: &Workspace, colors: &TermColors) -> Option<TermSnap> {
    let term = ws.terminal_ref()?;
    let (rows, cols) = term.size();
    let scrollback = term.scrollback_pos();
    let alive = term.alive;
    let snap = term.with_screen(|screen| {
        let mut out = Vec::with_capacity(rows as usize);
        for r in 0..rows {
            let mut text = String::with_capacity(cols as usize);
            let mut runs: Vec<Run> = Vec::new();
            for c in 0..cols {
                let (contents, mut fg, mut bg_raw, underline, inverse) = match screen.cell(r, c) {
                    Some(cell) => (
                        cell.contents().to_string(),
                        colors.vt_color(cell.fgcolor(), colors.fg),
                        colors.vt_color(cell.bgcolor(), colors.bg),
                        cell.underline(),
                        cell.inverse(),
                    ),
                    None => (String::new(), colors.fg, colors.bg, false, false),
                };
                if inverse {
                    std::mem::swap(&mut fg, &mut bg_raw);
                }
                let bg = (bg_raw != colors.bg).then_some(bg_raw);
                let piece = if contents.is_empty() { " " } else { &contents };
                text.push_str(piece);
                let len = piece.len();
                match runs.last_mut() {
                    Some(last) if last.fg == fg && last.bg == bg && last.underline == underline => {
                        last.len += len;
                    }
                    _ => runs.push(Run {
                        len,
                        fg,
                        bg,
                        underline,
                    }),
                }
            }
            out.push(RowSnap { text, runs });
        }
        let cursor = (!screen.hide_cursor()).then(|| screen.cursor_position());
        (out, cursor)
    })?;
    Some(TermSnap {
        rows: snap.0,
        cursor: snap.1,
        scrollback,
        alive,
        grid: (rows, cols),
    })
}

pub struct TermArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub focus: &'a FocusHandle,
    pub focused: bool,
    pub colors: &'a TermColors,
    pub resize_slot: ResizeSlot,
}

/// The terminal panel filling the chat area (egui placement parity).
pub fn terminal_panel(args: &TermArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let Some(snap) = snapshot(args.ws, args.colors) else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(t.weak)
            .child("Starting shell\u{2026}")
            .into_any_element();
    };
    let banner = (snap.scrollback > 0).then(|| {
        div()
            .absolute()
            .top_1()
            .right_2()
            .text_size(px(10.5))
            .text_color(t.paused)
            .child(format!(
                "\u{2191} {} (scroll to bottom to resume)",
                snap.scrollback
            ))
    });
    let dead = (!snap.alive).then(|| {
        div()
            .absolute()
            .bottom_1()
            .right_2()
            .text_size(px(10.5))
            .text_color(t.error)
            .child("shell exited")
    });

    let root_keys = args.root.clone();
    let root_scroll = args.root.clone();
    let focus_for_click = args.focus.clone();
    let canvas_el = grid_canvas(
        snap,
        args.colors.clone(),
        args.focused,
        args.resize_slot.clone(),
        id,
    );

    div()
        .id(("terminal", id.0))
        .relative()
        .flex_1()
        .min_h_0()
        .bg(args.colors.bg)
        .border_1()
        .border_color(if args.focused {
            alpha(t.accent, 0.6)
        } else {
            t.line_soft
        })
        .font_family("JetBrains Mono")
        .text_size(px(12.5))
        .track_focus(args.focus)
        .key_context("Terminal")
        .cursor(gpui::CursorStyle::IBeam)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
            window.focus(&focus_for_click);
        })
        .on_key_down(move |ev: &KeyDownEvent, _, cx| {
            if let Some(bytes) = key_to_bytes(ev, cx) {
                root_keys.update(cx, |r, cx| r.dispatch(DashAction::TermInput(id, bytes), cx));
            }
        })
        .on_scroll_wheel(move |ev: &ScrollWheelEvent, _, cx| {
            let lines = match ev.delta {
                gpui::ScrollDelta::Lines(p) => p.y,
                gpui::ScrollDelta::Pixels(p) => f32::from(p.y) / 18.0,
            };
            let delta = lines.round() as i32;
            if delta != 0 {
                root_scroll.update(cx, |r, cx| {
                    r.dispatch(DashAction::TermScroll(id, delta), cx)
                });
            }
        })
        .child(canvas_el)
        .children(banner)
        .children(dead)
        .into_any_element()
}

/// Keystroke -> PTY bytes: the shared named-key table, Ctrl-chords, paste
/// (cmd-V), printables via key_char. Cmd-chords (other than paste) pass
/// through to the app.
fn key_to_bytes(ev: &KeyDownEvent, cx: &mut gpui::App) -> Option<Vec<u8>> {
    let ks = &ev.keystroke;
    if ks.modifiers.platform {
        if ks.key == "v" {
            let text = cx.read_from_clipboard().and_then(|i| i.text())?;
            return Some(text.into_bytes());
        }
        return None;
    }
    if ks.modifiers.control
        && ks.key.len() == 1
        && let Some(b) = ctrl_byte(ks.key.chars().next().unwrap())
    {
        return Some(vec![b]);
    }
    if let Some(seq) = named_key_seq(&ks.key) {
        return Some(seq.to_vec());
    }
    if !ks.modifiers.control
        && let Some(ch) = &ks.key_char
        && !ch.is_empty()
    {
        return Some(ch.clone().into_bytes());
    }
    None
}

/// The grid painter: one shaped line per row (multi-run colors), bg quads
/// per run, 1px underlines, the block cursor. Also measures the cell box
/// and records the wanted rows/cols into the resize slot.
fn grid_canvas(
    snap: TermSnap,
    colors: TermColors,
    focused: bool,
    resize_slot: ResizeSlot,
    id: WorkspaceId,
) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let line_height = window.line_height();
            // Cell width from one shaped 'M' (mono font).
            let probe = window.text_system().shape_line(
                SharedString::from("M"),
                font_size,
                &[TextRun {
                    len: 1,
                    font: style.font(),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let cell_w = f32::from(probe.width).max(1.0);
            let cell_h = f32::from(line_height).max(1.0);

            // Record the grid size this box wants (applied next render).
            let want_cols = ((f32::from(bounds.size.width) - 4.0) / cell_w).floor() as u16;
            let want_rows = ((f32::from(bounds.size.height) - 4.0) / cell_h).floor() as u16;
            let want = (want_rows.max(1), want_cols.max(1));
            if want != snap.grid
                && let Ok(mut slot) = resize_slot.lock()
            {
                *slot = Some((id, want.0, want.1));
            }

            let ox = f32::from(bounds.origin.x) + 2.0;
            let oy = f32::from(bounds.origin.y) + 2.0;

            for (r, row) in snap.rows.iter().enumerate() {
                let y = oy + r as f32 * cell_h;
                // Shape the whole row once with its color runs.
                let runs: Vec<TextRun> = row
                    .runs
                    .iter()
                    .map(|run| TextRun {
                        len: run.len,
                        font: style.font(),
                        color: run.fg.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    })
                    .collect();
                let line = window.text_system().shape_line(
                    SharedString::from(row.text.clone()),
                    font_size,
                    &runs,
                    None,
                );
                // Backgrounds + underlines per run (byte-offset x lookups).
                let mut byte = 0usize;
                for run in &row.runs {
                    let x0 = f32::from(line.x_for_index(byte));
                    let x1 = f32::from(line.x_for_index(byte + run.len));
                    if let Some(bg) = run.bg {
                        window.paint_quad(fill(
                            Bounds::new(point(px(ox + x0), px(y)), size(px(x1 - x0), px(cell_h))),
                            bg,
                        ));
                    }
                    if run.underline {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(ox + x0), px(y + cell_h - 1.0)),
                                size(px(x1 - x0), px(1.0)),
                            ),
                            run.fg,
                        ));
                    }
                    byte += run.len;
                }
                let _ = line.paint(point(px(ox), px(y)), line_height, window, cx);
            }

            // Block cursor: filled when focused, outline when not (egui).
            if snap.scrollback == 0
                && let Some((cr, cc)) = snap.cursor
            {
                let x = ox + cc as f32 * cell_w;
                let y = oy + cr as f32 * cell_h;
                let b = Bounds::new(point(px(x), px(y)), size(px(cell_w), px(cell_h)));
                if focused {
                    window.paint_quad(fill(b, alpha(colors.cursor, 0.55)));
                } else {
                    let mut q = fill(b, gpui::transparent_black());
                    q.border_widths = gpui::Edges::all(px(1.));
                    q.border_color = colors.cursor.into();
                    window.paint_quad(q);
                }
            }
        },
    )
    .size_full()
}

/// Toolbar/composer toggle helper (used by the chat toolbar + skins).
pub fn terminal_toggle_btn(
    t: &Tokens,
    id: WorkspaceId,
    on: bool,
    key: &'static str,
    root: &Entity<RootView>,
) -> AnyElement {
    let root = root.clone();
    div()
        .id((key, id.0))
        .px_2()
        .py_0p5()
        .rounded(px(7.))
        .text_size(px(11.5))
        .text_color(if on { t.accent } else { t.text })
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .child("\u{2328} Terminal")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::TermToggle(id), cx));
        })
        .into_any_element()
}
