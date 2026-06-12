//! The Workspace Chat screen: optional 232px file explorer (lazy tree via
//! the WorkspaceFs trait + the session Changes list) beside the chat column
//! (transcript + composer dock).

pub mod ask;
pub mod composer;
pub mod sessions;
pub mod transcript;

use std::collections::{HashMap, HashSet};
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
    /// Chosen avatar emoji (QW8) — RootView resolves the defaults.
    pub user_avatar: String,
    pub puppy_avatar: String,
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
    /// Remote-workspace creds push: toolbar confirm armed / push in flight.
    pub creds_armed: bool,
    pub creds_busy: bool,
    /// Thinking folds the user/auto-collapse has closed.
    pub collapsed_thinking: &'a HashSet<(u64, usize)>,
    /// Sessions browser overlay state (open when Some).
    pub sessions: Option<sessions::SessionsArgs<'a>>,
    // -- editor area + tree state --
    /// The active file tab's code input (if the active tab is a file).
    pub editor_input: Option<&'a Entity<ChatInput>>,
    /// Tab index awaiting a dirty-close second click.
    pub editor_close_confirm: Option<usize>,
    /// A/M/D markers per absolute path (ws.tree_markers()).
    pub markers: HashMap<PathBuf, char>,
    /// Open tree context panel target (path, is_dir) for THIS workspace.
    pub tree_menu: Option<(PathBuf, bool)>,
    /// Rename/new input (shown inside the context panel when an op is armed).
    pub tree_op_input: Option<&'a Entity<ChatInput>>,
    pub tree_op_armed: bool,
    /// Delete awaiting confirmation (path shown in the panel).
    pub tree_delete_pending: Option<PathBuf>,
    // -- git pass-through --
    pub commit_input: Option<&'a Entity<ChatInput>>,
    pub git_list_mode: bool,
    pub graph_menu: Option<&'a (String, String, Vec<String>)>,
    pub branch_input: Option<&'a Entity<ChatInput>>,
    pub branch_armed: bool,
    pub creds_user_input: Option<&'a Entity<ChatInput>>,
    pub creds_pass_input: Option<&'a Entity<ChatInput>>,
    pub term_focus: &'a gpui::FocusHandle,
    pub term_focused: bool,
    pub term_colors: &'a crate::gpui_ui::terminal::TermColors,
    pub term_resize: crate::gpui_ui::terminal::ResizeSlot,
}

/// The whole chat screen body (below the tab strip).
pub fn chat_screen(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    // Terminal mode: the terminal fills the chat area (egui parity —
    // chat_body swaps the transcript out entirely while show_terminal).
    let body = if args.ws.terminal_visible() {
        crate::gpui_ui::terminal::terminal_panel(&crate::gpui_ui::terminal::TermArgs {
            t,
            ws: args.ws,
            root: args.root.clone(),
            focus: args.term_focus,
            focused: args.term_focused,
            colors: args.term_colors,
            resize_slot: args.term_resize.clone(),
        })
    } else {
        transcript::transcript_panel(&transcript::TranscriptArgs {
            t,
            ws: args.ws,
            root: args.root.clone(),
            puppy: args.puppy.clone(),
            user_avatar: args.user_avatar.clone(),
            puppy_avatar: args.puppy_avatar.clone(),
            show_all: args.show_all,
            expanded: args.expanded,
            collapsed_thinking: args.collapsed_thinking,
            reduce_motion: args.reduce_motion,
        })
    };
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
        puppy_avatar: args.puppy_avatar.clone(),
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
                .child(crate::gpui_ui::editor::editor_area(
                    &crate::gpui_ui::editor::EditorArgs {
                        t,
                        ws: args.ws,
                        root: args.root.clone(),
                        active_input: args.editor_input,
                        close_confirm: args.editor_close_confirm,
                        commit_input: args.commit_input,
                        git_list_mode: args.git_list_mode,
                        graph_menu: args.graph_menu,
                        branch_input: args.branch_input,
                        branch_armed: args.branch_armed,
                    },
                ))
                .child(body)
                .children(args.logs_open.then(|| logs_panel(args)))
                .child(answer)
                .child(dock),
        )
        .children(args.sessions.as_ref().map(sessions::sessions_overlay))
        .child(crate::gpui_ui::gitpanel::creds_overlay(
            &crate::gpui_ui::gitpanel::CredsArgs {
                t,
                ws: args.ws,
                root: args.root.clone(),
                user_input: args.creds_user_input,
                pass_input: args.creds_pass_input,
            },
        ))
        .into_any_element()
}

/// Slim workspace toolbar: + New chat / Sessions / spacer / logs toggle
/// (the mock's workspace-toolbar position; Explorer keeps its rail toggle).
fn ws_toolbar(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let can_new = ws.is_ready() && !ws.is_running_turn() && !ws.entries().is_empty();
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
    let git_btn = {
        let root = args.root.clone();
        div()
            .id(("ws-git", id.0))
            .px_2()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(11.5))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child("\u{2387} Git")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::ShowGit(id), cx));
            })
    };
    // Remote workspace only: push local auth + models to its host
    // (two-step confirm — it's credentials; results land as a transcript
    // note + toast).
    let creds_btn = |args: &ChatArgs| {
        let root = args.root.clone();
        let t = args.t;
        let host = args.ws.remote_label().unwrap_or_default().to_string();
        let (label, color) = if args.creds_busy {
            ("pushing creds\u{2026}".to_string(), t.weak)
        } else if args.creds_armed {
            (format!("send creds to {host}?"), t.accent)
        } else {
            ("push creds".to_string(), t.weak)
        };
        let busy = args.creds_busy;
        div()
            .id(("ws-push-creds", id.0))
            .px_2()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(11.5))
            .text_color(color)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .tooltip(crate::gpui_ui::widgets::text_tip(format!(
                "Push local code-puppy auth + model config to {host} \
                 (oauth tokens chmod 600; nothing is logged)"
            )))
            .child(label)
            .on_click(move |_, _, cx| {
                if busy {
                    return;
                }
                root.update(cx, |r, cx| r.dispatch(DashAction::PushCreds(id), cx));
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
        .children(args.ws.remote_fallback().then(|| {
            // Mode label, always visible in fallback chats (with the
            // honest capability note on hover).
            div()
                .px_1p5()
                .py_0p5()
                .rounded(px(6.))
                .border_1()
                .border_color(t.line_soft)
                .text_size(px(10.))
                .text_color(t.weak)
                .id(("ws-fallback-badge", id.0))
                .tooltip(crate::gpui_ui::widgets::text_tip(
                    "SSH-fallback: the sidecar runs locally; the agent works \
                     on the remote via ssh commands. Tree/editor/git/terminal \
                     are over ssh."
                        .into(),
                ))
                .child("ssh-fallback")
        }))
        .children(args.ws.is_remote().then(|| creds_btn(args)))
        .children(args.ws.is_git_repo().then_some(git_btn))
        .child(crate::gpui_ui::terminal::terminal_toggle_btn(
            &t,
            id,
            args.ws.terminal_visible(),
            "ws-term",
            &args.root,
        ))
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
        .children(tree_op_panel(args))
        .children(rows)
        .into_any_element()
}

/// Context panel for a right-clicked tree row: New file / New folder (dirs),
/// Rename, Delete (click-again confirm), with the inline name input while a
/// rename/new is armed. Replaces egui's three modals — same operations.
fn tree_op_panel(args: &ChatArgs) -> Option<AnyElement> {
    let t = args.t;
    let id = args.ws.id;
    let (path, is_dir) = args.tree_menu.clone()?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let mk = |label: &str, key: &'static str, action: DashAction, root: &Entity<RootView>| {
        let root = root.clone();
        div()
            .id((key, id.0))
            .px_1p5()
            .py_0p5()
            .rounded(px(6.))
            .text_size(px(10.5))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child(label.to_string())
            .on_click(move |_, _, cx| {
                let a = action.clone();
                root.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .into_any_element()
    };
    let mut row: Vec<AnyElement> = Vec::new();
    if is_dir {
        row.push(mk(
            "\u{ff0b} file",
            "tree-new-file",
            DashAction::TreeNew(id, path.clone(), false),
            &args.root,
        ));
        row.push(mk(
            "\u{ff0b} folder",
            "tree-new-dir",
            DashAction::TreeNew(id, path.clone(), true),
            &args.root,
        ));
    }
    row.push(mk(
        "rename",
        "tree-rename",
        DashAction::TreeRename(id, path.clone()),
        &args.root,
    ));
    let deleting = args.tree_delete_pending.as_ref() == Some(&path);
    row.push({
        let root = args.root.clone();
        let action = if deleting {
            DashAction::TreeDeleteConfirm
        } else {
            DashAction::TreeDelete(id, path.clone(), is_dir)
        };
        div()
            .id(("tree-delete", id.0))
            .px_1p5()
            .py_0p5()
            .rounded(px(6.))
            .text_size(px(10.5))
            .text_color(t.error)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child(if deleting { "sure? delete" } else { "delete" })
            .on_click(move |_, _, cx| {
                let a = action.clone();
                root.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .into_any_element()
    });
    row.push(mk(
        "\u{2715}",
        "tree-menu-close",
        DashAction::CloseChatPop,
        &args.root,
    ));

    Some(
        div()
            .mx_1()
            .mb_1()
            .p_1p5()
            .rounded(px(8.))
            .bg(t.well)
            .border_1()
            .border_color(alpha_accent(&t))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(10.))
                    .text_color(t.weak)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(name),
            )
            .child(div().flex().flex_wrap().gap_0p5().children(row))
            .children(
                (args.tree_op_armed && args.tree_op_input.is_some()).then(|| {
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(6.))
                        .bg(t.card)
                        .border_1()
                        .border_color(alpha_accent(&t))
                        .font_family("JetBrains Mono")
                        .text_size(px(11.))
                        .child(args.tree_op_input.unwrap().clone())
                }),
            )
            .into_any_element(),
    )
}

fn alpha_accent(t: &crate::gpui_ui::Tokens) -> gpui::Rgba {
    crate::gpui_ui::widgets::alpha(t.accent, 0.5)
}

/// Append one directory's rows (and recursively, its expanded children).
/// Depth is capped defensively; listings come from the TTL-cached fs.
fn push_dir_rows(args: &ChatArgs, dir: &std::path::Path, depth: usize, rows: &mut Vec<AnyElement>) {
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
        let marker = args.markers.get(&entry.path).copied();
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
            .child(div().min_w_0().flex_1().child(entry.name.clone()))
            .children(marker.map(|m| {
                div()
                    .flex_none()
                    .font_family("JetBrains Mono")
                    .text_size(px(10.))
                    .text_color(crate::gpui_ui::editor::marker_color(&t, m))
                    .child(m.to_string())
            }));
        // Left-click: dirs toggle, files open in the editor. Right-click:
        // the context panel (rename/new/delete) for either.
        let click_action = if entry.is_dir {
            DashAction::ToggleDir(id, entry.path.clone())
        } else {
            DashAction::OpenEditorFile(id, entry.path.clone())
        };
        let menu_pop = ChatPop::TreeMenu(id, entry.path.clone(), entry.is_dir);
        let root_click = args.root.clone();
        let root_menu = args.root.clone();
        let row = row
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .on_click(move |_, _, cx| {
                let a = click_action.clone();
                root_click.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .on_mouse_down(gpui::MouseButton::Right, move |_, _, cx| {
                let pop = menu_pop.clone();
                root_menu.update(cx, |r, cx| r.dispatch(DashAction::ToggleChatPop(pop), cx));
            })
            .into_any_element();
        rows.push(row);
        if open {
            push_dir_rows(args, &entry.path, depth + 1, rows);
        }
    }
}

/// The Changes list pinned under the tree: git working-tree changes when
/// the folder is a repo (click -> git diff in the Changes tab), else the
/// Code-Puppy-reported diffs (+adds/−dels).
fn changes_panel(args: &ChatArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    if args.ws.is_git_repo() {
        let changes = args.ws.git_change_list();
        return div()
            .flex_none()
            .max_h(px(180.))
            .id(("changes-scroll", id.0))
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
                    .child(format!("CHANGES ({})", changes.len())),
            )
            .children((changes.is_empty()).then(|| {
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(t.dim)
                    .child("Working tree clean.")
            }))
            .children(changes.iter().take(60).enumerate().map(|(i, c)| {
                let root = args.root.clone();
                let path = c.path.clone();
                let marker = c.marker;
                div()
                    .id(("git-change-row", i as u64))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .font_family("JetBrains Mono")
                    .text_size(px(10.5))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.well))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .text_color(crate::gpui_ui::editor::marker_color(&t, marker))
                            .child(marker.to_string()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(t.text)
                            .child(c.path.clone()),
                    )
                    .on_click(move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::LoadGitChange(id, path.clone(), marker), cx)
                        });
                    })
                    .into_any_element()
            }))
            .into_any_element();
    }
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
                .enumerate()
                .rev()
                .take(40)
                .map(|(ix, d)| {
                    let root = args.root.clone();
                    div()
                        .id(("diff-change-row", ix as u64))
                        .cursor_pointer()
                        .hover(|x| x.bg(t.well))
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::LoadDiffIndex(id, ix), cx)
                            });
                        })
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
    // (tab title, active?) when the browser surface has been opened.
    browser: Option<(String, bool)>,
    root: &Entity<RootView>,
) -> AnyElement {
    let on_dash = active_chat.is_none() && !den_active && !browser.as_ref().is_some_and(|b| b.1);
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
        .children(browser.map(|(title, active)| {
            let root = root.clone();
            div()
                .id("tab-browser")
                .px_2p5()
                .py_1()
                .rounded(px(8.))
                .text_size(px(12.))
                .cursor_pointer()
                .when(active, |d| {
                    d.bg(t.card)
                        .text_color(t.text)
                        .border_1()
                        .border_color(t.line_soft)
                })
                .when(!active, |d| d.text_color(t.weak))
                .flex()
                .items_center()
                .gap_1()
                .child(format!("\u{1f310} {title}"))
                .child({
                    // Close: stop the plugin + dismiss the surface (the
                    // tab used to be permanent once opened).
                    let root = root.clone();
                    div()
                        .id("tab-browser-close")
                        .px_0p5()
                        .text_size(px(10.))
                        .text_color(t.weak)
                        .hover(|d| d.text_color(t.text))
                        .child("\u{2715}")
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            root.update(cx, |r, cx| {
                                r.dispatch(
                                    DashAction::Browser(
                                        crate::gpui_ui::browser_ui::BrowserAction::CloseSurface,
                                    ),
                                    cx,
                                )
                            });
                        })
                })
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(
                            DashAction::Browser(crate::gpui_ui::browser_ui::BrowserAction::Open),
                            cx,
                        )
                    });
                })
        }))
        .into_any_element()
}
