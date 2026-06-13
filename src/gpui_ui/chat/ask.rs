//! The "Needs you" answer panel, rendered between transcript and composer
//! while an agent is blocked: either an `ask_user_question` request
//! (multi-question, radio/checkbox options, free-text Other) or a plain
//! input/confirm/select prompt. Answers flow through the SAME Workspace
//! methods the egui modal uses (`ask_submit` / `pending_choose` / ...).

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::{PendingKind, Workspace};

pub struct AskArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    /// Shared single-line answer input ("Other..." / input prompts).
    pub answer_input: Option<&'a Entity<ChatInput>>,
    /// Which ask-question index the answer input is bound to (Other text).
    pub other_target: Option<usize>,
    pub reduce_motion: bool,
    /// Chosen puppy avatar emoji (QW8).
    pub puppy_avatar: String,
}

/// The whole panel; empty when nothing is blocked.
pub fn ask_panel(args: &AskArgs) -> AnyElement {
    if let Some(ask) = args.ws.ask_state() {
        return panel_frame(
            &args.t,
            format!(
                "{} {} asks",
                crate::gpui_ui::avatars::inline(
                    &args.puppy_avatar,
                    crate::gpui_ui::avatars::PUPPY_DEFAULT
                ),
                args.ws.name
            ),
            ask_body(args, ask),
            args.reduce_motion,
        );
    }
    if let Some(pending) = args.ws.pending_request() {
        return panel_frame(
            &args.t,
            format!(
                "{} {} needs you",
                crate::gpui_ui::avatars::inline(
                    &args.puppy_avatar,
                    crate::gpui_ui::avatars::PUPPY_DEFAULT
                ),
                args.ws.name
            ),
            pending_body(args, pending),
            args.reduce_motion,
        );
    }
    div().into_any_element()
}

fn panel_frame(t: &Tokens, title: String, body: AnyElement, reduce_motion: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .mx_4()
        .mb_2()
        .px_3()
        .py_2p5()
        .rounded(px(12.))
        .bg(alpha(t.wait, 0.07))
        .border_1()
        .border_color(alpha(t.wait, 0.55))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(widgets::status_dot(
                    u64::MAX - 1,
                    t.wait,
                    true,
                    reduce_motion,
                ))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.wait)
                        .child(title),
                ),
        )
        .child(body)
        .into_any_element()
}

/// ask_user_question: every question with its options + Other + Submit/Cancel.
fn ask_body(args: &AskArgs, ask: &crate::workspace::AskState) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let root = &args.root;
    div()
        .flex()
        .flex_col()
        .gap_2p5()
        .children(ask.questions.iter().enumerate().map(|(qi, q)| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(q.header.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.weak)
                        .child(q.question.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1p5()
                        .children(q.options.iter().enumerate().map(|(oi, opt)| {
                            let on = q.selected.get(oi).copied().unwrap_or(false);
                            let root = root.clone();
                            let glyph = match (q.multi_select, on) {
                                (true, true) => "\u{2611}",
                                (true, false) => "\u{2610}",
                                (false, true) => "\u{25c9}",
                                (false, false) => "\u{25cb}",
                            };
                            div()
                                .id(("ask-opt", (qi * 64 + oi) as u64))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_2p5()
                                .py_1()
                                .rounded(px(8.))
                                .bg(t.well)
                                .border_1()
                                .border_color(if on {
                                    alpha(t.accent, 0.7)
                                } else {
                                    t.line_soft
                                })
                                .text_size(px(12.))
                                .text_color(if on { t.text } else { t.weak })
                                .cursor_pointer()
                                .hover(|d| d.border_color(alpha(t.accent, 0.5)))
                                .child(div().text_color(t.accent).child(glyph))
                                .child(opt.label.clone())
                                .on_click(move |_, _, cx| {
                                    root.update(cx, |r, cx| {
                                        r.dispatch(DashAction::AskToggle(id, qi, oi), cx)
                                    });
                                })
                        }))
                        .child(other_toggle(args, qi)),
                )
                .children((args.other_target == Some(qi)).then(|| other_row(args)))
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    widgets::primary_btn(&t, "Submit")
                        .id(("ask-submit", id.0))
                        .on_click({
                            let root = root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| r.dispatch(DashAction::AskSubmit(id), cx));
                            }
                        }),
                )
                .child(
                    widgets::btn(&t, "Cancel")
                        .id(("ask-cancel", id.0))
                        .on_click({
                            let root = root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| r.dispatch(DashAction::AskCancel(id), cx));
                            }
                        }),
                ),
        )
        .into_any_element()
}

/// "Other..." chip: opens the shared answer input bound to question `qi`.
fn other_toggle(args: &AskArgs, qi: usize) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let on = args.other_target == Some(qi);
    let root = args.root.clone();
    div()
        .id(("ask-other", qi as u64))
        .px_2p5()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(if on {
            alpha(t.accent, 0.7)
        } else {
            t.line_soft
        })
        .text_size(px(12.))
        .text_color(t.weak)
        .cursor_pointer()
        .hover(|d| d.border_color(alpha(t.accent, 0.5)))
        .child("Other\u{2026}")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::AskOther(id, qi), cx));
        })
        .into_any_element()
}

fn other_row(args: &AskArgs) -> AnyElement {
    let t = args.t;
    let Some(input) = args.answer_input else {
        return div().into_any_element();
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(alpha(t.accent, 0.4))
        .child(div().min_w_0().flex_1().child(input.clone()))
        .into_any_element()
}

/// Plain input/confirm/select prompts: click-to-answer options or a text row.
fn pending_body(args: &AskArgs, pending: &crate::workspace::Pending) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let root = &args.root;
    match &pending.kind {
        PendingKind::Input { prompt, password } => div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(t.text)
                    .child(prompt.clone()),
            )
            .children(password.then(|| {
                div()
                    .text_size(px(10.5))
                    .text_color(t.paused)
                    .child("(password \u{2014} typed text is sent as-is)")
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(match args.answer_input {
                        Some(input) => div()
                            .min_w_0()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .rounded(px(8.))
                            .bg(t.well)
                            .border_1()
                            .border_color(alpha(t.accent, 0.4))
                            .child(input.clone())
                            .into_any_element(),
                        None => div().into_any_element(),
                    })
                    .child(
                        widgets::primary_btn(&t, "Answer")
                            .id(("pending-send", id.0))
                            .on_click({
                                let root = root.clone();
                                move |_, _, cx| {
                                    root.update(cx, |r, cx| {
                                        r.dispatch(DashAction::PendingText(id), cx)
                                    });
                                }
                            }),
                    ),
            )
            .into_any_element(),
        PendingKind::Confirm {
            title,
            description,
            options,
        } => choices(args, title, Some(description), options),
        PendingKind::Select { prompt, options } => choices(args, prompt, None, options),
    }
}

fn choices(
    args: &AskArgs,
    title: &str,
    description: Option<&str>,
    options: &[String],
) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .text_size(px(12.5))
                .text_color(t.text)
                .child(title.to_string()),
        )
        .children(description.filter(|d| !d.is_empty()).map(|d| {
            div()
                .text_size(px(11.5))
                .text_color(t.weak)
                .child(d.to_string())
        }))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1p5()
                .children(options.iter().enumerate().map(|(i, opt)| {
                    let root = args.root.clone();
                    widgets::btn(&t, opt.clone())
                        .id(("pending-opt", i as u64))
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::PendingChoose(id, i), cx)
                            });
                        })
                })),
        )
        .into_any_element()
}
