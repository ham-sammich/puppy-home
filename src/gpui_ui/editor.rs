//! The editor area: tab bar (files + Changes), syntax-highlighted editable
//! file views (code-mode ChatInput), and the Changes diff viewer.
//!
//! Highlight discipline: syntect runs ONCE per content change (root-driven,
//! 200 KB cap), producing [`SyntaxRuns`] consumed by the input's cached
//! layout — never per frame. Syntaxes/themes are syntect's bundled defaults
//! (same sets egui_extras ships); colors come from the theme's foreground
//! styles over our own background.

use std::path::Path;
use std::sync::OnceLock;

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Rgba, Styled as _, div,
    prelude::*, px,
};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::gpui_ui::input::{ChatInput, SyntaxRuns};
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::{EditorItem, Workspace, language_for};

/// Files above this size render unhighlighted (syntect on every keystroke
/// would hurt; egui has no cap but memoizes per frame — documented deviation).
pub const HIGHLIGHT_MAX_BYTES: usize = 200_000;

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The bundled syntect theme matching the app's light/dark mode. Always
/// using the dark one painted pastel-on-white in light themes (B13.2).
fn theme(dark: bool) -> &'static Theme {
    static DARK: OnceLock<Theme> = OnceLock::new();
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    let (slot, names): (&OnceLock<Theme>, [&str; 2]) = if dark {
        (&DARK, ["base16-eighties.dark", "base16-mocha.dark"])
    } else {
        (&LIGHT, ["base16-ocean.light", "InspiredGitHub"])
    };
    slot.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        set.themes
            .remove(names[0])
            .or_else(|| set.themes.remove(names[1]))
            .unwrap_or_default()
    })
}

/// Compute per-line syntax color runs for a file (None above the size cap
/// or for unknown languages — the editor then renders plain text).
pub fn highlight(content: &str, path: &Path, dark: bool) -> Option<SyntaxRuns> {
    if content.len() > HIGHLIGHT_MAX_BYTES {
        return None;
    }
    let set = syntax_set();
    let syntax = set
        .find_syntax_by_token(language_for(path))
        .or_else(|| set.find_syntax_by_token("txt"))?;
    let mut hl = HighlightLines::new(syntax, theme(dark));
    let mut lines = Vec::new();
    for line in content.split('\n') {
        let mut runs: Vec<(usize, gpui::Hsla)> = Vec::new();
        // Feed with a newline for correct parser state across lines, then
        // drop the trailing byte from the produced ranges.
        let with_nl = format!("{line}\n");
        match hl.highlight_line(&with_nl, set) {
            Ok(ranges) => {
                let mut remaining = line.len();
                for (style, text) in ranges {
                    if remaining == 0 {
                        break;
                    }
                    let len = text.len().min(remaining);
                    remaining -= len;
                    let f = style.foreground;
                    let color: gpui::Hsla = Rgba {
                        r: f.r as f32 / 255.0,
                        g: f.g as f32 / 255.0,
                        b: f.b as f32 / 255.0,
                        a: 1.0,
                    }
                    .into();
                    runs.push((len, color));
                }
            }
            Err(_) => runs.clear(),
        }
        lines.push(runs);
    }
    Some(std::sync::Arc::new(lines))
}

// ---------------------------------------------------------------------------
// Editor area UI
// ---------------------------------------------------------------------------

pub struct EditorArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    /// The active file tab's input entity (created by the root on open).
    pub active_input: Option<&'a Entity<ChatInput>>,
    /// Pending dirty-close confirmation for a tab index.
    pub close_confirm: Option<usize>,
    /// Browser plugin installed? (gates the HTML "Open in browser" button.)
    pub browser_available: bool,
    // -- git surface pass-through (gitpanel) --
    pub commit_input: Option<&'a Entity<ChatInput>>,
    pub git_list_mode: bool,
    pub graph_menu: Option<&'a (String, String, Vec<String>)>,
    pub branch_input: Option<&'a Entity<ChatInput>>,
    pub branch_armed: bool,
}

impl<'a> EditorArgs<'a> {
    fn git_args(&self) -> crate::gpui_ui::gitpanel::GitArgs<'a> {
        crate::gpui_ui::gitpanel::GitArgs {
            t: self.t,
            ws: self.ws,
            root: self.root.clone(),
            commit_input: self.commit_input,
            list_mode: self.git_list_mode,
            graph_menu: self.graph_menu,
            branch_input: self.branch_input,
            branch_armed: self.branch_armed,
        }
    }
}

/// The whole editor area (tab bar + active tab content). Empty when no tabs.
pub fn editor_area(args: &EditorArgs) -> AnyElement {
    let tabs = args.ws.editor_tabs();
    if tabs.is_empty() {
        return div().into_any_element();
    }
    let t = args.t;
    let active = args.ws.editor_active_ix();
    let content: AnyElement = match tabs.get(active) {
        Some(EditorItem::File(path)) => file_view(args, path),
        Some(EditorItem::Changes) => changes_viewer(args),
        Some(EditorItem::Git) => crate::gpui_ui::gitpanel::git_view(&args.git_args()),
        Some(EditorItem::Commit { hash, .. }) => {
            crate::gpui_ui::gitpanel::commit_view(&args.git_args(), hash)
        }
        Some(EditorItem::Browser(_)) => div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.weak)
            .child("Browser tabs land in Phase E.")
            .into_any_element(),
        None => div().into_any_element(),
    };
    div()
        .flex_none()
        .h(px(380.))
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(t.line_soft)
        .child(tab_bar(args))
        .child(div().flex_1().min_h_0().flex().flex_col().child(content))
        .into_any_element()
}

fn tab_bar(args: &EditorArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let active = ws.editor_active_ix();
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .px_1()
        .py_0p5()
        .bg(t.panel)
        .border_b_1()
        .border_color(t.line_soft)
        .children(ws.editor_tabs().iter().enumerate().map(|(i, item)| {
            let (label, dirty) = match item {
                EditorItem::File(p) => (
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string()),
                    ws.is_file_dirty(p),
                ),
                EditorItem::Changes => ("Changes".to_string(), false),
                EditorItem::Git => ("Git".to_string(), false),
                EditorItem::Browser(_) => ("Browser".to_string(), false),
                EditorItem::Commit { .. } => ("Commit".to_string(), false),
            };
            let on = i == active;
            let confirm = args.close_confirm == Some(i);
            let root_focus = args.root.clone();
            let root_close = args.root.clone();
            div()
                .id(("editor-tab", i as u64))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded(px(6.))
                .text_size(px(11.5))
                .font_family("JetBrains Mono")
                .cursor_pointer()
                .when(on, |d| d.bg(t.card).text_color(t.text))
                .when(!on, |d| d.text_color(t.weak).hover(|d| d.bg(t.well)))
                .child(label)
                .children(dirty.then(|| div().text_color(t.paused).child("\u{25cf}")))
                .child(
                    div()
                        .id(("editor-tab-x", i as u64))
                        .px_0p5()
                        .text_color(if confirm { t.error } else { t.dim })
                        .hover(|d| d.text_color(t.error))
                        .child(if confirm { "sure?" } else { "\u{2715}" })
                        .on_click(move |_, _, cx| {
                            root_close
                                .update(cx, |r, cx| r.dispatch(DashAction::EditorClose(id, i), cx));
                        }),
                )
                .on_click(move |_, _, cx| {
                    root_focus.update(cx, |r, cx| r.dispatch(DashAction::EditorTab(id, i), cx));
                })
                .into_any_element()
        }))
        .into_any_element()
}

/// One open file: path bar (dirty marker + Save) over the code input in a
/// both-axes scroll container.
fn file_view(args: &EditorArgs, path: &Path) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let Some((_, dirty, load_err, save_err)) = ws.file_view(path) else {
        return div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.weak)
            .child("file not open")
            .into_any_element();
    };

    let bar = div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_0p5()
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .font_family("JetBrains Mono")
                .text_size(px(10.5))
                .text_color(t.weak)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(path.display().to_string()),
        )
        .children(dirty.then(|| {
            div()
                .text_size(px(10.5))
                .text_color(t.paused)
                .child("\u{25cf} unsaved")
        }))
        .children(ws.is_git_repo().then(|| {
            let on = ws.blame_enabled(path);
            let root = args.root.clone();
            let p = path.to_path_buf();
            widgets::btn(&t, "\u{1f50d} Blame")
                .when(on, |d| {
                    d.border_color(crate::gpui_ui::widgets::alpha(t.accent, 0.8))
                })
                .id(("editor-blame", id.0))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::BlameToggle(id, p.clone()), cx)
                    });
                })
                .into_any_element()
        }))
        .children((args.browser_available && crate::browser::is_html(path)).then(|| {
            // Preview a local HTML file in the browser plugin (egui parity).
            let root = args.root.clone();
            let url = crate::browser::file_url(path);
            widgets::btn(&t, "\u{1f310} Open in browser")
                .id(("editor-html-open", id.0))
                .tooltip(widgets::text_tip(
                    "Preview this HTML file in the in-app browser".into(),
                ))
                .on_click(move |_, _, cx| {
                    let url = url.clone();
                    root.update(cx, |r, cx| r.dispatch(DashAction::OpenDevUrl(url), cx));
                })
        }))
        .child(
            widgets::btn(&t, "\u{1f4be} Save")
                .id(("editor-save", id.0))
                .on_click({
                    let root = args.root.clone();
                    let path = path.to_path_buf();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::EditorSave(id, path.clone()), cx)
                        });
                    }
                }),
        );

    let body: AnyElement = if ws.blame_enabled(path) {
        crate::gpui_ui::gitpanel::blame_view(&t, ws, path)
    } else if let Some(err) = load_err {
        div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.error)
            .child(format!("Cannot open file: {err}"))
            .into_any_element()
    } else if let Some(input) = args.active_input {
        div()
            .id(("editor-scroll", id.0))
            .flex_1()
            .min_h_0()
            // flex-col so the code input's flex_grow fills the viewport: a
            // short file's empty area below the text is still clickable to
            // place the caret / focus (#feedback: click-to-focus).
            .flex()
            .flex_col()
            .overflow_x_scroll()
            .overflow_y_scroll()
            .font_family("JetBrains Mono")
            .text_size(px(12.))
            .bg(t.well)
            .child(input.clone())
            .into_any_element()
    } else {
        div().into_any_element()
    };

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(bar)
        .children(save_err.map(|err| {
            div()
                .px_2()
                .py_0p5()
                .text_size(px(11.))
                .text_color(t.error)
                .child(format!("Save failed: {err}"))
        }))
        .child(body)
        .into_any_element()
}

/// The Changes tab: changed-file list (left) + the selected colored diff.
fn changes_viewer(args: &EditorArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;

    // (label-path, marker, action) per changed file, from the same sources
    // the egui explorer uses: git working tree, else Code-Puppy diffs.
    let rows: Vec<(String, char, DashAction)> = if ws.is_git_repo() {
        ws.git_change_list()
            .iter()
            .map(|c| {
                (
                    c.path.clone(),
                    c.marker,
                    DashAction::LoadGitChange(id, c.path.clone(), c.marker),
                )
            })
            .collect()
    } else {
        ws.diff_changed_files()
            .into_iter()
            .map(|(ix, path, marker)| (path, marker, DashAction::LoadDiffIndex(id, ix)))
            .collect()
    };

    let list = div()
        .w(px(240.))
        .flex_none()
        .id(("changes-list", id.0))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .py_1()
        .border_r_1()
        .border_color(t.line_soft)
        .children((rows.is_empty()).then(|| {
            div()
                .px_2()
                .text_size(px(11.5))
                .text_color(t.dim)
                .child("No changes yet.")
        }))
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(i, (path, marker, action))| {
                    let selected = ws
                        .current_diff_view()
                        .map(|d| d.path == path)
                        .unwrap_or(false);
                    let root = args.root.clone();
                    div()
                        .id(("change-row", i as u64))
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_0p5()
                        .font_family("JetBrains Mono")
                        .text_size(px(11.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(alpha(t.accent, 0.12)))
                        .hover(|d| d.bg(t.well))
                        .child(
                            div()
                                .w(px(12.))
                                .flex_none()
                                .text_color(marker_color(&t, marker))
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
                                .child(path),
                        )
                        .on_click(move |_, _, cx| {
                            let a = action.clone();
                            root.update(cx, |r, cx| r.dispatch(a, cx));
                        })
                }),
        );

    let diff: AnyElement = match ws.current_diff_view() {
        Some(d) => div()
            .id(("diff-pane", id.0))
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1p5()
            .p_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(op_color(&t, &d.operation))
                            .child(d.operation.clone()),
                    )
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(11.5))
                            .text_color(t.text)
                            .child(d.path.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(11.))
                            .flex()
                            .gap_1()
                            .child(div().text_color(t.run).child(format!("+{}", d.adds)))
                            .child(
                                div()
                                    .text_color(t.error)
                                    .child(format!("\u{2212}{}", d.dels)),
                            ),
                    ),
            )
            .child(crate::gpui_ui::chat::transcript::diff_body(&t, &d.lines))
            .into_any_element(),
        None => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(t.dim)
            .child("Pick a file in the Changes list to see its diff.")
            .into_any_element(),
    };

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .child(list)
        .child(diff)
        .into_any_element()
}

/// A/M/D/R/? marker colors (mirrors the egui `marker_color` mapping).
pub fn marker_color(t: &Tokens, marker: char) -> Rgba {
    match marker {
        'A' | '?' => t.run,
        'D' => t.error,
        'R' => t.think,
        _ => t.paused, // 'M' and anything else: modified-amber
    }
}

fn op_color(t: &Tokens, op: &str) -> Rgba {
    match op {
        "create" => t.run,
        "delete" => t.error,
        _ => t.paused,
    }
}
