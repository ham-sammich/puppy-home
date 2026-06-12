//! The sessions browser: list saved Code Puppy sessions for a workspace,
//! preview one read-only, resume it here. Mirrors the egui modal
//! (`workspace/sessions.rs`): filter box, current-session marker, source
//! tag, message count, refresh, the "stop the turn first" warning. Egui has
//! no delete — neither do we (the sidecar exposes none).

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens, markdown};
use crate::workspace::{Workspace, short_session};

pub struct SessionsArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub filter_input: Option<&'a Entity<ChatInput>>,
    /// The filter input's current text (read by the caller, who has cx).
    pub filter: String,
    /// Currently selected `(name, source)`, if any.
    pub selected: Option<&'a (String, String)>,
    pub puppy: String,
    /// Chosen puppy avatar emoji (QW8).
    pub puppy_avatar: String,
}

/// Full-screen scrim + centered panel (deferred above everything).
pub fn sessions_overlay(args: &SessionsArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let root = &args.root;

    let panel = div()
        .occlude()
        .w(px(720.))
        .max_w_full()
        .h(px(480.))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(13.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child(format!(
                            "\u{1f5c2} Code Puppy sessions \u{2014} {}",
                            ws.name
                        )),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.weak)
                        .child(format!("{} session(s)", ws.sessions_catalog().len())),
                )
                .child(
                    widgets::btn(&t, "\u{27f3} Refresh")
                        .id("sess-refresh")
                        .on_click({
                            let root = root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| {
                                    r.dispatch(DashAction::SessionsRefresh(id), cx)
                                });
                            }
                        }),
                )
                .child(widgets::btn(&t, "Close").id("sess-close").on_click({
                    let root = root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| r.dispatch(DashAction::CloseSessions, cx));
                    }
                })),
        )
        .children(ws.is_running_turn().then(|| {
            div()
                .text_size(px(11.5))
                .text_color(t.paused)
                .child("\u{26a0} Stop the running turn before loading a session.")
        }))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .child(list_pane(args))
                .child(preview_pane(args)),
        );

    gpui::deferred(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(alpha(t.bg, 0.6))
            .child(panel),
    )
    .with_priority(200)
    .into_any_element()
}

/// LEFT: filter box + the (filtered) session list.
fn list_pane(args: &SessionsArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let needle = args.filter.to_ascii_lowercase();

    let matches = |s: &crate::backend::SessionInfo| -> bool {
        needle.is_empty()
            || short_session(&s.name)
                .to_ascii_lowercase()
                .contains(&needle)
            || s.source.to_ascii_lowercase().contains(&needle)
    };
    let catalog = ws.sessions_catalog();
    let shown = catalog.iter().filter(|s| matches(s)).count();

    div()
        .w(px(250.))
        .flex_none()
        .flex()
        .flex_col()
        .gap_1p5()
        .children(args.filter_input.map(|input| {
            div()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .text_size(px(11.5))
                .child(input.clone())
        }))
        .children((catalog.is_empty()).then(|| {
            div()
                .text_size(px(11.5))
                .text_color(t.dim)
                .child("No saved sessions found yet.")
        }))
        .children((!catalog.is_empty() && shown == 0).then(|| {
            div()
                .text_size(px(11.5))
                .text_color(t.dim)
                .child("No sessions match the filter.")
        }))
        .child(
            div()
                .id(("sess-list", id.0))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_0p5()
                .children(
                    catalog
                        .iter()
                        .filter(|s| matches(s))
                        .enumerate()
                        .map(|(i, s)| {
                            let current = s.name == ws.sessions_current_name();
                            let selected =
                                args.selected.map(|(n, _)| n == &s.name).unwrap_or(false);
                            let root = args.root.clone();
                            let name = s.name.clone();
                            let source = s.source.clone();
                            div()
                                .id(("sess-row", i as u64))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_2()
                                .py_1()
                                .rounded(px(7.))
                                .cursor_pointer()
                                .when(selected, |d| d.bg(alpha(t.accent, 0.14)))
                                .hover(|d| d.bg(t.well))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(t.text)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(format!(
                                                    "{}{}",
                                                    if current { "\u{25cf} " } else { "" },
                                                    short_session(&s.name)
                                                )),
                                        )
                                        .child(
                                            div()
                                                .font_family("JetBrains Mono")
                                                .text_size(px(9.5))
                                                .text_color(t.dim)
                                                .child(format!(
                                                    "{} \u{b7} {} msg",
                                                    s.source, s.messages
                                                )),
                                        ),
                                )
                                .on_click(move |_, _, cx| {
                                    root.update(cx, |r, cx| {
                                        r.dispatch(
                                            DashAction::SessionSelect(
                                                id,
                                                name.clone(),
                                                source.clone(),
                                            ),
                                            cx,
                                        )
                                    });
                                })
                                .into_any_element()
                        }),
                ),
        )
        .into_any_element()
}

/// RIGHT: read-only preview of the selected session + the Resume button.
fn preview_pane(args: &SessionsArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let Some((sel_name, sel_source)) = args.selected else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(t.dim)
            .child("Select a session to preview it.")
            .into_any_element();
    };

    let body: AnyElement = match ws.session_preview_data() {
        Some((name, entries)) if name == sel_name => div()
            .id(("sess-preview", id.0))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .children(entries.iter().take(200).map(|e| {
                if e.role == "user" {
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(10.5)).text_color(t.weak).child("you"))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.text)
                                .child(e.text.clone()),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(t.accent)
                                .child(format!("{} {}", args.puppy_avatar, args.puppy)),
                        )
                        .child(markdown::render(&t, &e.text))
                        .into_any_element()
                }
            }))
            .into_any_element(),
        _ => div()
            .flex_1()
            .text_size(px(12.))
            .text_color(t.dim)
            .child("Loading conversation\u{2026}")
            .into_any_element(),
    };

    let resume = {
        let root = args.root.clone();
        let name = sel_name.clone();
        let source = sel_source.clone();
        widgets::primary_btn(&t, "Resume here \u{2192}")
            .id("sess-resume")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(
                        DashAction::SessionResume(id, name.clone(), source.clone()),
                        cx,
                    )
                });
            })
    };

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.5))
                        .text_color(t.text)
                        .child(short_session(sel_name)),
                )
                .child(div().flex_1())
                .child(resume),
        )
        .child(body)
        .into_any_element()
}
