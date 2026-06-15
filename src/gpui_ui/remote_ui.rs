//! The remote-connect dialog render (egui `views/remote_connect.rs`
//! window at parity) + the E5 path-browser listing panel. State, dispatch
//! and the worker threads live in `remote`.

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers_ui::{disabled_btn, small};
use crate::gpui_ui::remote::{RemoteAction, RemoteState};
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};

/// Click handler funneling a remote action through the root dispatch.
fn ract(
    root: &Entity<RootView>,
    a: RemoteAction,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    let root = root.clone();
    move |_, _, cx| {
        let a = a.clone();
        root.update(cx, |r, cx| r.dispatch(DashAction::Remote(a), cx));
    }
}

/// A labelled framed input row (the dialog's two text fields).
fn input_row(t: &Tokens, label: &str, input: Option<&Entity<ChatInput>>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(small(t, label, t.weak))
        .children(input.map(|i| {
            div()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .text_size(px(11.5))
                .child(i.clone())
        }))
        .into_any_element()
}

/// The centered connect dialog; rendered from the root when `remote` is set.
pub(crate) fn overlay(
    t: Tokens,
    root: &Entity<RootView>,
    st: &RemoteState,
    inputs: &[Entity<ChatInput>],
    target_text: &str,
    path_text: &str,
    push_busy: bool,
) -> AnyElement {
    let ready = !target_text.trim().is_empty() && !path_text.trim().is_empty();
    let has_target = !target_text.trim().is_empty();

    // "puppush": send local auth + model config to the host — usable
    // before/with Connect. Two-step confirm (credentials), worker runs on
    // a thread, result arrives as a toast.
    let push_section: AnyElement = if push_busy {
        small(&t, "Pushing auth + models\u{2026}", t.weak).into_any_element()
    } else if st.push_confirm {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(small(
                &t,
                format!(
                    "Send your auth tokens + model config to {}?",
                    target_text.trim()
                ),
                t.text,
            ))
            .child(
                widgets::primary_btn(&t, "Push now")
                    .id("remote-push-confirm")
                    .on_click(ract(root, RemoteAction::PushCreds)),
            )
            .child(
                widgets::btn(&t, "Cancel")
                    .id("remote-push-cancel")
                    .on_click(ract(root, RemoteAction::PushCredsCancel)),
            )
            .into_any_element()
    } else {
        div()
            .child(if has_target {
                widgets::btn(&t, "Push my auth + models to this host\u{2026}")
                    .id("remote-push")
                    .tooltip(widgets::text_tip(
                        "Copy local code-puppy OAuth tokens (chmod 600) and model \
                         config to the host's ~/.code_puppy"
                            .into(),
                    ))
                    .on_click(ract(root, RemoteAction::PushCreds))
                    .into_any_element()
            } else {
                disabled_btn(&t, "Push my auth + models to this host\u{2026}").into_any_element()
            })
            .into_any_element()
    };

    // Hosts from ~/.ssh/config (clicking one fills the target field).
    let hosts: AnyElement = if st.hosts.is_empty() {
        small(&t, "No hosts found in ~/.ssh/config.", t.dim).into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(small(&t, "Hosts from your SSH config:", t.weak))
            .child(
                div()
                    .id("remote-hosts")
                    .max_h(px(120.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(st.hosts.iter().enumerate().map(|(i, host)| {
                        let selected = target_text == host.as_str();
                        div()
                            .id(("remote-host", i as u64))
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(alpha(t.accent, 0.12)))
                            .hover(|d| d.bg(t.well))
                            .text_size(px(12.))
                            .font_family("JetBrains Mono")
                            .text_color(t.text)
                            .child(host.clone())
                            .on_click(ract(root, RemoteAction::HostPick(host.clone())))
                    })),
            )
            .into_any_element()
    };

    // Path entry: typed field + Browse, or the live folder browser.
    let path_section: AnyElement = if let Some(b) = &st.browser {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(small(&t, "Pick a remote folder:", t.weak))
            .child(listing(
                &t,
                root,
                &b.cwd,
                &b.entries,
                b.pending.is_some(),
                b.error.as_deref(),
            ))
            .child(
                div().child(
                    widgets::btn(&t, "Cancel browse")
                        .id("remote-browse-cancel")
                        .on_click(ract(root, RemoteAction::BrowseCancel)),
                ),
            )
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(input_row(&t, "Remote folder path:", inputs.get(1)))
            .child(
                div().child(
                    widgets::btn(&t, "Browse the remote host\u{2026}")
                        .id("remote-browse")
                        .tooltip(widgets::text_tip(
                            "Pick a folder by browsing the remote filesystem".into(),
                        ))
                        .on_click(ract(root, RemoteAction::BrowseOpen)),
                ),
            )
            .into_any_element()
    };

    // Footer: Connecting state replaces the buttons (egui behavior).
    let footer: AnyElement = if st.connecting {
        small(&t, "Connecting over SSH\u{2026}", t.weak).into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                widgets::btn(&t, "Cancel")
                    .id("remote-cancel")
                    .on_click(ract(root, RemoteAction::Close)),
            )
            .child(div().flex_1())
            .child(if ready {
                widgets::primary_btn(&t, "Connect")
                    .id("remote-connect")
                    .on_click(ract(root, RemoteAction::Connect))
                    .into_any_element()
            } else {
                disabled_btn(&t, "Connect").into_any_element()
            })
            .into_any_element()
    };

    let panel = div()
        .occlude()
        .w(px(500.))
        .max_w_full()
        .max_h(px(620.))
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
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child("Connect to a remote folder"),
                )
                .child(div().flex_1())
                .children((!st.connecting).then(|| {
                    widgets::btn(&t, "Close")
                        .id("remote-close")
                        .on_click(ract(root, RemoteAction::Close))
                })),
        )
        // Header pinned above; body scrolls so the footer (Connect) never
        // gets pushed off-screen at small window sizes (F2/F3).
        .child(
            div()
                .id("remote-body-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .child(hosts)
                .child(input_row(
                    &t,
                    "SSH target  ( [user@]host[:port] ):",
                    inputs.first(),
                ))
                .child(path_section)
                .child(push_section)
                .children(st.fallback_offer.as_ref().map(|launcher| {
                    // CannotHost verdict: offer SSH-fallback mode explicitly.
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .p_2()
                        .rounded(px(9.))
                        .bg(alpha(t.accent, 0.08))
                        .border_1()
                        .border_color(alpha(t.accent, 0.4))
                        .child(small(
                            &t,
                            format!(
                                "Remote can't run Code Puppy (`{launcher}` not found). \
                         Connect in SSH-fallback mode? Your LOCAL puppy will \
                         operate on the project via ssh commands; tree, editor, \
                         git and terminal still work over ssh."
                            ),
                            t.text,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    widgets::primary_btn(&t, "Connect in SSH-fallback mode")
                                        .id("remote-fallback-go")
                                        .on_click(ract(root, RemoteAction::ConnectFallback)),
                                )
                                .child(
                                    widgets::btn(&t, "Cancel")
                                        .id("remote-fallback-no")
                                        .on_click(ract(root, RemoteAction::FallbackDismiss)),
                                ),
                        )
                }))
                .children(st.error.clone().map(|e| small(&t, e, t.error))),
        )
        .child(footer);

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
    .with_priority(230)
    .into_any_element()
}

/// E5: the shared directory-listing panel (egui `path_browser` in dir-pick
/// mode): ".. up" + mono cwd header, folders-first alphabetical entries,
/// "(empty)", inline error, "Use this folder".
fn listing(
    t: &Tokens,
    root: &Entity<RootView>,
    cwd: &str,
    entries: &[(String, bool)],
    loading: bool,
    error: Option<&str>,
) -> AnyElement {
    let mut dirs: Vec<&str> = entries
        .iter()
        .filter(|(_, is_dir)| *is_dir)
        .map(|(n, _)| n.as_str())
        .collect();
    dirs.sort_unstable();
    let empty = entries.is_empty() && !loading;

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    widgets::btn(t, ".. up")
                        .id("remote-up")
                        .tooltip(widgets::text_tip("Parent folder".into()))
                        .on_click(ract(root, RemoteAction::BrowseUp)),
                )
                .child(
                    div()
                        .font_family("JetBrains Mono")
                        .text_size(px(11.))
                        .text_color(t.weak)
                        .child(cwd.to_string()),
                )
                // Static label, not a spinner: decorative motion stays out.
                .children(loading.then(|| small(t, "loading\u{2026}", t.dim))),
        )
        .children(error.map(|e| small(t, e.to_string(), t.error)))
        .child(
            div()
                .id("remote-listing")
                .max_h(px(300.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(dirs.iter().enumerate().map(|(i, name)| {
                    div()
                        .id(("remote-dir", i as u64))
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|d| d.bg(t.well))
                        .font_family("JetBrains Mono")
                        .text_size(px(12.))
                        .text_color(t.text)
                        .child(format!("{name}/"))
                        .on_click(ract(root, RemoteAction::BrowseEnter((*name).to_string())))
                }))
                .children(empty.then(|| small(t, "(empty)", t.dim))),
        )
        .child(
            div().child(
                widgets::btn(t, "Use this folder")
                    .id("remote-use-folder")
                    .tooltip(widgets::text_tip(
                        "Open this directory in the workspace".into(),
                    ))
                    .on_click(ract(root, RemoteAction::BrowsePick)),
            ),
        )
        .into_any_element()
}
