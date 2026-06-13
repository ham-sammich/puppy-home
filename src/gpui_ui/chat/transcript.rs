//! The chat transcript: turns with avatars + who-lines, markdown bodies,
//! tool-call chips (`edit · path ·  +A −D`), collapsed diff blocks, thinking
//! folds, and the breathing-puppy empty state.
//!
//! Render cost is bounded exactly like redesign/egui: only the most recent
//! [`RENDER_TAIL`] entries render unless "Show older" was clicked. The column
//! is a normal `flex_col` (oldest→newest, top→bottom) tracking a per-workspace
//! `ScrollHandle`; RootView pins it to the bottom on new turns while still
//! letting you scroll up to read history. (An earlier `flex_col_reverse`
//! trick fought gpui's scroll range — older turns landed at negative
//! coordinates the scroll offset couldn't reach, so you could scroll *past*
//! the newest into blank but never *up* to history.)

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Entity, FontWeight, IntoElement, ParentElement as _,
    Styled as _, div, ease_in_out, prelude::*, px,
};

use crate::backend::BackendMessage;
use crate::gpui_ui::widgets::alpha;
use crate::gpui_ui::{DashAction, RootView, Tokens, markdown};
use crate::workspace::{Entry, Workspace, diff};

/// Max transcript entries rendered by default (egui parity).
pub const RENDER_TAIL: usize = 120;

pub struct TranscriptArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub puppy: String,
    /// Chosen avatar emoji (QW8) — RootView resolves the defaults.
    pub user_avatar: String,
    pub puppy_avatar: String,
    pub show_all: bool,
    /// The transcript scroll handle (bottom-pinned by RootView).
    pub scroll: gpui::ScrollHandle,
    /// `(workspace id, entry index)` pairs whose diff body is open.
    pub expanded: &'a std::collections::HashSet<(u64, usize)>,
    /// Thinking folds that are CLOSED (default open while streaming; the
    /// turn-end one-shot auto-collapses via the drain loop — egui parity).
    pub collapsed_thinking: &'a std::collections::HashSet<(u64, usize)>,
    pub reduce_motion: bool,
}

pub fn transcript_panel(args: &TranscriptArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let entries = ws.entries();
    let total = entries.len();

    if total == 0 && ws.collapsed_count() == 0 {
        return empty_state(&args.t, &args.puppy, &args.puppy_avatar, args.reduce_motion);
    }

    let start = if args.show_all {
        0
    } else {
        total.saturating_sub(RENDER_TAIL)
    };

    // Children top→bottom = oldest→newest: the trimmed/older notices first
    // (top), then the rendered entries in chronological order.
    let mut children: Vec<AnyElement> = Vec::with_capacity(total - start + 2);
    if ws.collapsed_count() > 0 {
        children.push(
            div()
                .text_size(px(11.5))
                .text_color(t.weak)
                .child(format!(
                    "{} earlier message(s) trimmed to keep the UI responsive.",
                    ws.collapsed_count()
                ))
                .into_any_element(),
        );
    }
    if start > 0 {
        let root = args.root.clone();
        let id = ws.id;
        children.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.5))
                .text_color(t.weak)
                .child(format!("{start} older message(s) hidden for speed."))
                .child(
                    div()
                        .id(("show-older", id.0))
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.))
                        .bg(t.well)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.text))
                        .child("Show older")
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| r.dispatch(DashAction::ShowOlder(id), cx));
                        }),
                )
                .into_any_element(),
        );
    }
    for (i, entry) in entries.iter().enumerate().skip(start) {
        children.push(render_entry(args, i, entry));
    }

    div()
        .id(("chat-scroll", ws.id.0))
        .track_scroll(&args.scroll)
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_3()
        .children(children)
        .into_any_element()
}

fn render_entry(args: &TranscriptArgs, idx: usize, entry: &Entry) -> AnyElement {
    let t = args.t;
    match entry {
        Entry::User(text) => turn(
            &t,
            &args.user_avatar,
            who(&t, "you", None),
            markdown_plain(&t, text),
        ),
        Entry::Agent(text) => turn(
            &t,
            &args.puppy_avatar,
            agent_who(&t, args),
            markdown::render(&t, text),
        ),
        Entry::Note(text) => div()
            .text_size(px(12.))
            .text_color(t.weak)
            .italic()
            .child(text.clone())
            .into_any_element(),
        Entry::Error(text) => div()
            .text_size(px(12.5))
            .text_color(t.error)
            .child(format!("\u{26a0} {text}"))
            .into_any_element(),
        Entry::Message(msg) => render_message(args, idx, msg),
        Entry::Thinking { text, .. } => {
            // Open by default while streaming; the drain loop consumes the
            // turn-end collapse signal into `collapsed_thinking`; manual
            // toggles win thereafter.
            let open = !args.collapsed_thinking.contains(&(args.ws.id.0, idx));
            let root = args.root.clone();
            let id = args.ws.id;
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .id(("think-toggle", idx as u64))
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(12.))
                        .text_color(t.dim)
                        .italic()
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.weak))
                        .child(format!(
                            "\u{1f4ad} thinking\u{2026} {}",
                            if open { "\u{25be}" } else { "\u{25b8}" }
                        ))
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::ToggleThinking(id, idx), cx)
                            });
                        }),
                )
                .when(open, |d| {
                    d.child(
                        div()
                            .pl_4()
                            .text_size(px(12.))
                            .text_color(t.dim)
                            .italic()
                            .child(text.clone()),
                    )
                })
                .into_any_element()
        }
    }
}

/// Backend messages: agent prose, diff chips, tool output, system noise.
fn render_message(args: &TranscriptArgs, idx: usize, msg: &BackendMessage) -> AnyElement {
    let t = args.t;
    if msg.category == "agent" {
        return turn(
            &t,
            &args.puppy_avatar,
            agent_who(&t, args),
            markdown::render(&t, &msg.text),
        );
    }
    if msg.kind == "DiffMessage"
        && let Some((path, adds, dels)) = diff_chip_info(msg)
    {
        let open = args.expanded.contains(&(args.ws.id.0, idx));
        let root = args.root.clone();
        let id = args.ws.id;
        return div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id(("diff-chip", idx as u64))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        root.update(cx, |r, cx| r.dispatch(DashAction::ToggleDiff(id, idx), cx));
                    })
                    .child(tool_chip(&t, "edit", Some(path), Some((adds, dels)))),
            )
            .when(open, |d| {
                // Full line parsing only happens while the body is open.
                match diff::parse_diff(msg) {
                    Some(rec) => d.child(diff_body(&t, &rec.lines)),
                    None => d.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.weak)
                            .child("diff body unavailable"),
                    ),
                }
            })
            .into_any_element();
    }
    if msg.category == "tool_output" {
        return div()
            .flex()
            .items_start()
            .gap_2()
            .child(tool_chip(&t, &tool_label(&msg.kind), None, None))
            .child(clamped_text(&t, &msg.text))
            .into_any_element();
    }
    let color = match msg.category.as_str() {
        "user_interaction" => t.wait,
        "divider" => t.dim,
        _ => t.weak,
    };
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(10.5))
                .text_color(color)
                .child(format!("[{}]", msg.kind)),
        )
        .child(clamped_text(&t, &msg.text))
        .into_any_element()
}

/// A turn row: 30px avatar + (who line, body).
fn turn(t: &Tokens, emoji: &str, who: AnyElement, body: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_2p5()
        .child(
            div()
                .size(px(30.))
                .flex_none()
                .rounded(px(9.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(crate::gpui_ui::avatars::fill_parent(emoji, 15., 9.)),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(who)
                .child(body),
        )
        .into_any_element()
}

fn who(t: &Tokens, name: &str, tag: Option<String>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .text_size(px(11.))
                .text_color(t.weak)
                .child(name.to_string()),
        )
        .children(tag.map(|tg| {
            div()
                .font_family("JetBrains Mono")
                .text_size(px(9.5))
                .text_color(t.dim)
                .child(tg)
        }))
        .into_any_element()
}

fn agent_who(t: &Tokens, args: &TranscriptArgs) -> AnyElement {
    let tag = format!("{} \u{b7} {}", args.ws.agent, args.ws.model);
    who(t, &args.puppy, Some(tag))
}

fn markdown_plain(t: &Tokens, text: &str) -> AnyElement {
    div()
        .text_size(px(13.))
        .text_color(t.text)
        .child(text.to_string())
        .into_any_element()
}

/// ` label · detail  +A −D` chip.
fn tool_chip(
    t: &Tokens,
    label: &str,
    detail: Option<&str>,
    counts: Option<(usize, usize)>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded(px(7.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .text_size(px(11.))
        .child(div().child("\u{1f527}"))
        .child(div().text_color(t.text).child(label.to_string()))
        .children(detail.map(|d| {
            div()
                .font_family("JetBrains Mono")
                .text_color(t.weak)
                .max_w(px(260.))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(d.to_string())
        }))
        .children(counts.map(|(a, d)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .font_family("JetBrains Mono")
                .child(div().text_color(t.run).child(format!("\u{2713} +{a}")))
                .child(div().text_color(t.error).child(format!("\u{2212}{d}")))
        }))
        .into_any_element()
}

/// Colored add/remove/context rows for an opened diff (capped at 200 rows).
pub(crate) fn diff_body(t: &Tokens, lines: &[diff::DiffLine]) -> AnyElement {
    const MAX_ROWS: usize = 200;
    div()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .px_2p5()
        .py_1p5()
        .font_family("JetBrains Mono")
        .text_size(px(11.5))
        .children(lines.iter().take(MAX_ROWS).map(|l| {
            let (sigil, color, bg) = match l.kind.as_str() {
                "add" => ("+", t.run, Some(alpha(t.run, 0.07))),
                "remove" => ("\u{2212}", t.error, Some(alpha(t.error, 0.07))),
                _ => (" ", t.dim, None),
            };
            let mut row = div()
                .flex()
                .gap_2()
                .px_1()
                .whitespace_nowrap()
                .overflow_x_hidden()
                .child(div().w(px(10.)).flex_none().text_color(color).child(sigil))
                .child(div().text_color(color).child(l.content.clone()));
            if let Some(bg) = bg {
                row = row.bg(bg);
            }
            row
        }))
        .children((lines.len() > MAX_ROWS).then(|| {
            div()
                .text_color(t.weak)
                .child(format!("\u{2026} {} more lines", lines.len() - MAX_ROWS))
        }))
        .into_any_element()
}

fn clamped_text(t: &Tokens, text: &str) -> AnyElement {
    const MAX: usize = 700;
    let shown = if text.len() > MAX {
        let cut = text
            .char_indices()
            .take_while(|(i, _)| *i < MAX)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}\u{2026}", &text[..cut])
    } else {
        text.to_string()
    };
    div()
        .min_w_0()
        .flex_1()
        .font_family("JetBrains Mono")
        .text_size(px(11.5))
        .text_color(t.weak)
        .child(shown)
        .into_any_element()
}

/// Path + add/del counts from a `DiffMessage` payload (cheap per-frame pass;
/// the full line list parses lazily when the body opens). Egui parity.
fn diff_chip_info(msg: &BackendMessage) -> Option<(&str, usize, usize)> {
    let p = &msg.payload;
    let path = p.get("file_path").and_then(serde_json::Value::as_str)?;
    let mut adds = 0;
    let mut dels = 0;
    if let Some(arr) = p.get("diff_lines").and_then(serde_json::Value::as_array) {
        for l in arr {
            match l.get("kind").and_then(serde_json::Value::as_str) {
                Some("add") => adds += 1,
                Some("remove") => dels += 1,
                _ => {}
            }
        }
    }
    Some((path, adds, dels))
}

/// Friendly names for tool-output kinds (egui parity table).
fn tool_label(kind: &str) -> String {
    match kind {
        "AgentReasoning" => "reasoning".into(),
        "ToolOutput" => "tool".into(),
        "CommandOutput" => "shell".into(),
        other => other.to_lowercase(),
    }
}

/// Centered breathing puppy + zzz + "How can {puppy} help you?".
fn empty_state(t: &Tokens, puppy: &str, avatar: &str, reduce_motion: bool) -> AnyElement {
    let dog: AnyElement = if reduce_motion {
        div()
            .child(crate::gpui_ui::avatars::boxed(avatar, 66., 16.))
            .into_any_element()
    } else {
        div()
            .child(crate::gpui_ui::avatars::boxed(avatar, 66., 16.))
            .with_animation(
                "empty-bob",
                Animation::new(Duration::from_millis(4200))
                    .repeat()
                    .with_easing(ease_in_out),
                |el, delta| {
                    let bob = 1.0 - (delta * 2.0 - 1.0).abs();
                    el.mt(px(8.0 * bob))
                },
            )
            .into_any_element()
    };
    let zzz: AnyElement = if reduce_motion {
        div()
            .text_size(px(13.))
            .text_color(t.dim)
            .child("z z z")
            .into_any_element()
    } else {
        div()
            .text_size(px(13.))
            .text_color(t.dim)
            .child("z z z")
            .with_animation(
                "empty-zzz",
                Animation::new(Duration::from_millis(4200))
                    .repeat()
                    .with_easing(ease_in_out),
                |el, delta| {
                    let f = 1.0 - (delta * 2.0 - 1.0).abs();
                    el.opacity(0.3 + 0.7 * f).mb(px(6.0 * f))
                },
            )
            .into_any_element()
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(zzz)
        .child(dog)
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::BOLD)
                .text_color(t.text)
                .child(format!("How can {puppy} help you?")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.weak)
                .child("Ask anything about this workspace \u{2014} or drop a / command."),
        )
        .into_any_element()
}
