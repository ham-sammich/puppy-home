//! The Workspace Chat screen: optional 232px file explorer (lazy tree via
//! the WorkspaceFs trait + the session Changes list) beside the chat column
//! (transcript + composer dock).

pub mod ask;
pub mod composer;
pub mod sessions;
pub mod transcript;

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{AnyElement, Entity, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::{ChatPop, DashAction, RootView, Tokens};
use crate::session::ComposerStyle;
use crate::workspace::Workspace;

pub struct ChatArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub input: Entity<ChatInput>,
    pub style: ComposerStyle,
    pub pop: Option<&'a ChatPop>,
    pub puppy: String,
    pub show_all: bool,
    pub expanded: &'a HashSet<(u64, usize)>,
    pub reduce_motion: bool,
    pub tree_open: bool,
    pub expanded_dirs: &'a HashSet<(u64, PathBuf)>,
    /// Shared answer input (ask Other / pending input prompts).
    pub answer_input: Option<&'a Entity<ChatInput>>,
    /// The ask question index the answer input is bound to, if any.
    pub other_target: Option<usize>,
    /// Pending pasted images `(index, thumbnail)`.
    pub images: Vec<(usize, std::sync::Arc<gpui::Image>)>,
    /// Completion-palette keyboard selection.
    pub palette_sel: usize,
    /// Dock steer toggle (false = now, true = queue).
    pub steer_queue: bool,
    /// Logs panel visibility.
    pub logs_open: bool,
    /// Thinking folds the user/auto-collapse has closed.
    pub collapsed_thinking: &'a HashSet<(u64, usize)>,
    /// Sessions browser overlay state (open when Some).
    pub sessions: Option<sessions::SessionsArgs<'a>>,
}

/// The whole chat screen body (below the tab strip).
pub fn chat_screen(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let body = transcript::transcript_panel(&transcript::TranscriptArgs {
        t,
        ws: args.ws,
        root: args.root.clone(),
        puppy: args.puppy.clone(),
        show_all: args.show_all,
        expanded: args.expanded,
        collapsed_thinking: args.collapsed_thinking,
        reduce_motion: args.reduce_motion,
    });
    let dock = composer::composer_dock(&composer::ComposerArgs {
        t,
        ws: args.ws,
        root: args.root.clone(),
        input: args.input.clone(),
        style: args.style,
        pop: args.pop,
        puppy: args.puppy.clone(),
        images: args.images.clone(),
        palette_sel: args.palette_sel,
        steer_queue: args.steer_queue,
    });

    let answer = ask::ask_panel(&ask::AskArgs {
        t,
        ws: args.ws,
        root: args.root.clone(),
        answer_input: args.answer_input,
        other_target: args.other_target,
        reduce_motion: args.reduce_motion,
    });

    div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .child(explorer_or_rail(args))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .rounded(px(12.))
                .border_1()
                .border_color(t.line_soft)
                .bg(t.card)
                .overflow_hidden()
                .child(ws_toolbar(args))
                .child(body)
                .children(args.logs_open.then(|| logs_panel(args)))
                .child(answer)
                .child(dock),
        )
        .children(args.sessions.as_ref().map(sessions::sessions_overlay))
        .into_any_element()
}

/// Slim workspace toolbar: + New chat / Sessions / spacer / logs toggle
/// (the mock's workspace-toolbar position; Explorer keeps its rail toggle).
fn ws_toolbar(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let can_new = ws.is_ready() && !ws.is_running_turn() && ws.entries().is_empty() == false;
    let new_chat = {
        let root = args.root.clone();
        div()
            .id(("ws-new-chat", id.0))
            .px_2()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(11.5))
            .text_color(if can_new { t.text } else { t.dim })
            .when(can_new, |d| d.cursor_pointer().hover(|d| d.bg(t.well)))
            .child("\u{ff0b} New chat")
            .when(can_new, |d| {
                d.on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| r.dispatch(DashAction::NewChat(id), cx));
                })
            })
    };
    let sessions_btn = {
        let root = args.root.clone();
        div()
            .id(("ws-sessions", id.0))
            .px_2()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(11.5))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child("\u{1f5c2} Sessions")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::OpenSessions(id), cx));
            })
    };
    let logs_btn = {
        let root = args.root.clone();
        let on = args.logs_open;
        div()
            .id(("ws-logs", id.0))
            .px_2()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(11.5))
            .text_color(if on { t.accent } else { t.weak })
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child("logs")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::ToggleLogs(id), cx));
            })
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .border_b_1()
        .border_color(t.line_soft)
        .child(new_chat)
        .child(sessions_btn)
        .child(div().flex_1())
        .child(logs_btn)
        .into_any_element()
}

/// Sidecar log lines (stderr/events), bottom-pinned, bounded tail.
fn logs_panel(args: &ChatArgs) -> AnyElement {
    const LOG_TAIL: usize = 200;
    let t = args.t;
    let lines = args.ws.log_lines();
    let start = lines.len().saturating_sub(LOG_TAIL);
    div()
        .h(px(140.))
        .flex_none()
        .border_t_1()
        .border_color(t.line_soft)
        .bg(t.well)
        .flex()
        .flex_col()
        .child(
            div()
                .px_2()
                .py_0p5()
                .text_size(px(9.5))
                .text_color(t.weak)
                .child(format!("SIDECAR LOGS ({} lines)", lines.len())),
        )
        .child(
            div()
                .id(("ws-logs-scroll", args.ws.id.0))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col_reverse()
                .px_2()
                .pb_1()
                .font_family("JetBrains Mono")
                .text_size(px(10.))
                .text_color(t.weak)
                .children(
                    lines[start..]
                        .iter()
                        .rev()
                        .map(|l| div().whitespace_nowrap().child(l.clone())),
                ),
        )
        .into_any_element()
}

/// The explorer panel when open, or a slim rail with the toggle when closed.
fn explorer_or_rail(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let root = args.root.clone();
    let toggle = div()
        .id(("tree-toggle", id.0))
        .px_1p5()
        .py_0p5()
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(if args.tree_open { t.accent } else { t.weak })
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .child("\u{25a4}")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::ToggleTree(id), cx));
        });

    if !args.tree_open {
        return div().pr_2().child(toggle).into_any_element();
    }
    div()
        .w(px(232.))
        .flex_none()
        .mr_3()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(t.line_soft)
        .bg(t.card)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(t.weak)
                        .child("EXPLORER"),
                )
                .child(div().flex_1())
                .child(toggle),
        )
        .child(tree_panel(args))
        .child(changes_panel(args))
        .into_any_element()
}

/// Lazy directory tree: the root listing, expanding one level per click.
fn tree_panel(args: &ChatArgs) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    push_dir_rows(args, &args.ws.root.clone(), 0, &mut rows);
    div()
        .id(("tree-scroll", args.ws.id.0))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .py_1()
        .children(rows)
        .into_any_element()
}

/// Append one directory's rows (and recursively, its expanded children).
/// Depth is capped defensively; listings come from the TTL-cached fs.
fn push_dir_rows(args: &ChatArgs, dir: &PathBuf, depth: usize, rows: &mut Vec<AnyElement>) {
    const MAX_DEPTH: usize = 12;
    const MAX_ROWS: usize = 400;
    if depth > MAX_DEPTH || rows.len() > MAX_ROWS {
        return;
    }
    let t = args.t;
    let id = args.ws.id;
    let Ok(mut entries) = args.ws.fs_handle().read_dir(dir) else {
        return;
    };
    entries.sort_by(|a, b| {
        (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase()))
    });
    for entry in entries {
        if entry.name.starts_with('.') {
            continue; // dotfiles stay out of the lazy tree (egui parity-ish)
        }
        if rows.len() > MAX_ROWS {
            return;
        }
        let open = entry.is_dir && args.expanded_dirs.contains(&(id.0, entry.path.clone()));
        let glyph = if entry.is_dir {
            if open { "\u{25be}" } else { "\u{25b8}" }
        } else {
            "\u{b7}"
        };
        let row = div()
            .id(("tree-row", rows.len() as u64))
            .flex()
            .items_center()
            .gap_1()
            .pl(px(8.0 + depth as f32 * 12.0))
            .pr_2()
            .py_0p5()
            .text_size(px(11.5))
            .text_color(if entry.is_dir { t.text } else { t.weak })
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(div().w(px(10.)).flex_none().text_color(t.dim).child(glyph))
            .child(entry.name.clone());
        let row = if entry.is_dir {
            let root = args.root.clone();
            let path = entry.path.clone();
            row.cursor_pointer()
                .hover(|d| d.bg(t.well))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::ToggleDir(id, path.clone()), cx)
                    });
                })
                .into_any_element()
        } else {
            row.into_any_element()
        };
        rows.push(row);
        if open {
            push_dir_rows(args, &entry.path, depth + 1, rows);
        }
    }
}

/// The session Changes list pinned under the tree (+adds/−dels per file).
fn changes_panel(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let records = args.ws.diff_records();
    div()
        .flex_none()
        .max_h(px(180.))
        .id(("changes-scroll", args.ws.id.0))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(t.line_soft)
        .child(
            div()
                .px_2()
                .py_1()
                .text_size(px(10.5))
                .text_color(t.weak)
                .child(format!("CHANGES ({})", records.len())),
        )
        .children(if records.is_empty() {
            vec![
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(t.dim)
                    .child("No file changes this session.")
                    .into_any_element(),
            ]
        } else {
            records
                .iter()
                .rev()
                .take(40)
                .map(|d| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_0p5()
                        .font_family("JetBrains Mono")
                        .text_size(px(10.5))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(t.text)
                                .child(d.path.clone()),
                        )
                        .child(div().text_color(t.run).child(format!("+{}", d.adds)))
                        .child(
                            div()
                                .text_color(t.error)
                                .child(format!("\u{2212}{}", d.dels)),
                        )
                        .into_any_element()
                })
                .collect()
        })
        .into_any_element()
}

/// The tab strip: Dashboard + one tab per workspace (status dot, name,
/// close) + the Den tab while joined.
pub fn tab_strip(
    t: &Tokens,
    tabs: Vec<(crate::workspace::WorkspaceId, String, gpui::Rgba)>,
    active_chat: Option<crate::workspace::WorkspaceId>,
    den: Option<(String, bool)>,
    den_active: bool,
    root: &Entity<RootView>,
) -> AnyElement {
    let on_dash = active_chat.is_none() && !den_active;
    let dash_tab = {
        let root = root.clone();
        div()
            .id("tab-dashboard")
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(px(8.))
            .text_size(px(12.))
            .cursor_pointer()
            .when(on_dash, |d| {
                d.bg(t.card)
                    .text_color(t.text)
                    .border_1()
                    .border_color(t.line_soft)
            })
            .when(!on_dash, |d| d.text_color(t.weak))
            .child("\u{1f4ca} Dashboard")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::ShowDashboard, cx));
            })
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(dash_tab)
        .children(tabs.into_iter().map(|(id, name, color)| {
            let on = active_chat == Some(id);
            let root_open = root.clone();
            let root_close = root.clone();
            div()
                .id(("tab-ws", id.0))
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded(px(8.))
                .text_size(px(12.))
                .font_family("JetBrains Mono")
                .cursor_pointer()
                .when(on, |d| {
                    d.bg(t.card)
                        .text_color(t.text)
                        .border_1()
                        .border_color(t.line_soft)
                })
                .when(!on, |d| d.text_color(t.weak))
                .child(div().size(px(7.)).rounded_full().bg(color))
                .child(name)
                .child(
                    div()
                        .id(("tab-close", id.0))
                        .px_0p5()
                        .text_color(t.dim)
                        .hover(|d| d.text_color(t.error))
                        .child("\u{2715}")
                        .on_click(move |_, _, cx| {
                            root_close
                                .update(cx, |r, cx| r.dispatch(DashAction::CloseWorkspace(id), cx));
                        }),
                )
                .on_click(move |_, _, cx| {
                    root_open.update(cx, |r, cx| r.dispatch(DashAction::Open(id), cx));
                })
        }))
        .children(den.map(|(room, alive)| {
            let root_show = root.clone();
            let root_leave = root.clone();
            div()
                .id("tab-den")
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded(px(8.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(den_active, |d| {
                    d.bg(t.card)
                        .text_color(t.text)
                        .border_1()
                        .border_color(t.line_soft)
                })
                .when(!den_active, |d| d.text_color(t.weak))
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(if alive { t.run } else { t.error }),
                )
                .child(format!(
                    "\u{1f43e} {} \u{b7} {room}",
                    crate::pack::DEN_LABEL
                ))
                .child(
                    div()
                        .id("tab-den-close")
                        .px_0p5()
                        .text_color(t.dim)
                        .hover(|d| d.text_color(t.error))
                        .child("\u{2715}")
                        .on_click(move |_, _, cx| {
                            root_leave.update(cx, |r, cx| {
                                r.dispatch(
                                    DashAction::Den(crate::gpui_ui::den::DenAction::Leave),
                                    cx,
                                )
                            });
                        }),
                )
                .on_click(move |_, _, cx| {
                    root_show.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(crate::gpui_ui::den::DenAction::Show), cx)
                    });
                })
        }))
        .into_any_element()
}
