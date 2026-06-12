//! One agent card: header (avatar / identity / model pill + popover), state
//! line, last-prompt inset, stats row with mini sparkline, sub-agent rows,
//! inline steer / new-prompt input, and the state-dependent action bar.
//! Anatomy follows `pack-card.jsx`; vocabulary comes from `card_state`.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Entity, FocusHandle, FontWeight,
    IntoElement, KeyDownEvent, ParentElement as _, RenderOnce, Styled as _, Window, div,
    ease_in_out, prelude::*, px, relative,
};

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::InstanceStatus;

use super::{CardSnapshot, InputKind};

/// Boxed click handler (boxing keeps `AgentCard::on` free of `&self`
/// lifetime capture in its opaque return type).
type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;

#[derive(IntoElement)]
pub struct AgentCard {
    pub t: Tokens,
    pub snap: CardSnapshot,
    pub root: Entity<RootView>,
    /// This card's expanded inline input, if any: (kind, text, queue-mode).
    pub inline: Option<(InputKind, String, bool)>,
    pub input_focus: FocusHandle,
    pub reduce_motion: bool,
}

impl AgentCard {
    /// A click handler that funnels one action into `RootView::dispatch`.
    fn on(&self, action: DashAction) -> OnClick {
        let root = self.root.clone();
        Box::new(move |_, _, cx| {
            let a = action.clone();
            root.update(cx, |r, cx| r.dispatch(a, cx));
        })
    }

    fn header(&self) -> impl IntoElement {
        let t = self.t;
        let s = &self.snap;
        div()
            .flex()
            .items_center()
            .gap_2p5()
            .child(avatar(&t, s.emoji, s.live, s.id.0, self.reduce_motion))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(widgets::status_dot(
                                s.id.0,
                                s.color,
                                s.live,
                                self.reduce_motion,
                            ))
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text)
                                    .child(s.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(11.))
                            .text_color(t.weak)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(match &s.puppy {
                                // A foreign (remote-host) puppy: lead the
                                // meta line with it, subtle — full identity
                                // theater is reserved for the Den.
                                Some(p) => {
                                    format!("\u{1f436} {p} \u{b7} {} \u{b7} {}", s.agent, s.path)
                                }
                                None => format!("{} \u{b7} {}", s.agent, s.path),
                            }),
                    ),
            )
            .child(super::model_pill::model_pill(
                &self.t, &self.snap, &self.root,
            ))
    }

    /// State label + tool / question / error context + the right-side clock.
    fn state_line(&self) -> impl IntoElement {
        let t = self.t;
        let s = &self.snap;
        let context: AnyElement = match s.status {
            InstanceStatus::WaitingForInput => match &s.question {
                Some(q) => div()
                    .text_size(px(11.5))
                    .text_color(t.wait)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(q.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            InstanceStatus::Dead => div()
                .text_size(px(11.5))
                .text_color(t.error)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(s.status_line.clone())
                .into_any_element(),
            _ if s.live && s.tool.is_some() => div()
                .font_family("JetBrains Mono")
                .text_size(px(11.5))
                .text_color(t.weak)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(format!("\u{b7} {}", s.tool.clone().unwrap_or_default()))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(s.color)
                    .child(s.label),
            )
            .child(div().min_w_0().flex_1().child(context))
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .child(s.clock.clone()),
            )
    }

    /// Dark inset quoting the last user prompt + LAST PROMPT tag / +N queued.
    fn last_prompt(&self) -> AnyElement {
        let t = self.t;
        let s = &self.snap;
        if s.last_prompt.is_empty() {
            return div().into_any_element();
        }
        let mut tag = "LAST PROMPT".to_string();
        if s.queued > 0 {
            tag.push_str(&format!(" \u{b7} +{} queued", s.queued));
        }
        div()
            .id(("last-prompt", s.id.0))
            .flex()
            .items_center()
            .gap_2()
            .px_2p5()
            .py_1p5()
            .rounded(px(9.))
            .bg(t.well)
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(t.accent)
                    .child("\u{275d}"),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(t.text)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(s.last_prompt.clone()),
            )
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(9.5))
                    .text_color(t.dim)
                    .child(tag),
            )
            .tooltip(widgets::text_tip(s.last_prompt.clone()))
            .into_any_element()
    }

    /// The design's context-progress bar: 3px, gradient think→run, live cards
    /// only. Unknown ctx draws nothing — a 0% bar would be a lie.
    fn context_bar(&self) -> AnyElement {
        let t = self.t;
        let s = &self.snap;
        let Some(pct) = s.ctx_pct.filter(|_| s.live) else {
            return div().into_any_element();
        };
        let frac = (pct / 100.0).clamp(0.0, 1.0) as f32;
        div()
            .id(("ctx-bar", s.id.0))
            .h(px(3.))
            .w_full()
            .rounded_full()
            .bg(t.well)
            .child(
                div()
                    .h_full()
                    .w(relative(frac))
                    .rounded_full()
                    .bg(gpui::linear_gradient(
                        90.,
                        gpui::linear_color_stop(t.think, 0.),
                        gpui::linear_color_stop(t.run, 1.),
                    )),
            )
            .tooltip(widgets::text_tip(format!("context window {pct:.0}% full")))
            .into_any_element()
    }

    /// The five stat cells: tok/s (+ mini spark), tokens, tools, files, cost.
    fn stats_row(&self) -> impl IntoElement {
        let t = self.t;
        let s = &self.snap;
        let cell = |k: &str, v: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .flex_1()
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(t.weak)
                        .child(k.to_string()),
                )
                .child(v)
        };
        let mono = |txt: String| {
            div()
                .font_family("JetBrains Mono")
                .font_weight(FontWeight::BOLD)
                .text_size(px(12.5))
                .text_color(t.text)
                .child(txt)
                .into_any_element()
        };
        div()
            .flex()
            .gap_2()
            .child(cell(
                "tok/s",
                div()
                    .flex()
                    .items_end()
                    .gap_1p5()
                    .child(mono(format!("{:.0}", s.rate)))
                    .child(widgets::sparkline(s.sparks.clone(), 46.0, 16.0, t.run))
                    .into_any_element(),
            ))
            .child(cell("tokens", mono(widgets::fmt_k(s.tokens))))
            .child(cell("tools", mono(s.tools.to_string())))
            .child(cell(
                "files",
                div()
                    .flex()
                    .gap_1()
                    .font_family("JetBrains Mono")
                    .text_size(px(12.5))
                    .child(div().text_color(t.run).child(format!("+{}", s.adds)))
                    .child(
                        div()
                            .text_color(t.error)
                            .child(format!("\u{2212}{}", s.dels)),
                    )
                    .into_any_element(),
            ))
            .child(cell(
                "cost",
                // null = unknown: an honest dash, never $0.00. "≈" marks
                // values priced from the dated models.dev snapshot.
                mono(match s.cost {
                    Some(c) if s.cost_estimated => format!("\u{2248}${c:.2}"),
                    Some(c) => format!("${c:.2}"),
                    None => "\u{2014}".to_string(),
                }),
            ))
    }

    /// Nested `invoke_agent` rows behind a dashed-look left rule.
    fn sub_agents(&self) -> AnyElement {
        let t = self.t;
        let s = &self.snap;
        if s.subs.is_empty() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .ml_1()
            .pl_2p5()
            .border_l_1()
            .border_color(alpha(t.weak, 0.35))
            .children(s.subs.iter().map(|sa| {
                let mut state = sa.status.clone();
                if let Some(tool) = &sa.tool {
                    state.push_str(&format!(" \u{b7} {tool}"));
                }
                state.push_str(&format!(" \u{b7} {}t", sa.tools));
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(6.)).rounded_full().bg(sa.color))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.text)
                            .child(format!("\u{21b3} {} {}", sa.emoji, sa.name)),
                    )
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(10.5))
                            .text_color(t.weak)
                            .child(sa.model.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(sa.color)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(state),
                    )
            }))
            .into_any_element()
    }

    /// The expanded steer / new-prompt input. Minimal key handling (chars,
    /// backspace, paste, Enter, Escape) — the full IME-aware input lands with
    /// the 2.3 composer (see GPUI_NOTES.md).
    fn inline_input(&self) -> AnyElement {
        let t = self.t;
        let Some((kind, text, queue)) = &self.inline else {
            return div().into_any_element();
        };
        let id = self.snap.id;
        let (tag, hint, send_label) = match kind {
            InputKind::Steer => ("STEER", "Nudge this agent mid-task\u{2026}", "Nudge"),
            InputKind::Send => ("SEND", "Send a new prompt\u{2026}", "Send"),
        };
        let root = self.root.clone();
        let shown: AnyElement = if text.is_empty() {
            div()
                .text_color(t.dim)
                .child(hint.to_string())
                .into_any_element()
        } else {
            div()
                .text_color(t.text)
                .child(format!("{text}\u{258f}"))
                .into_any_element()
        };
        let key_root = self.root.clone();
        let row = div()
            .id(("card-input", id.0))
            .track_focus(&self.input_focus)
            .on_key_down(move |ev: &KeyDownEvent, _, cx| {
                let ks = &ev.keystroke;
                let action = if ks.key == "enter" {
                    Some(DashAction::SubmitInput)
                } else if ks.key == "escape" {
                    Some(DashAction::CloseInput)
                } else {
                    None
                };
                if let Some(a) = action {
                    key_root.update(cx, |r, cx| r.dispatch(a, cx));
                    return;
                }
                key_root.update(cx, |r, cx| r.edit_input(ks, cx));
            })
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(px(9.))
            .bg(t.well)
            .border_1()
            .border_color(alpha(t.accent, 0.5))
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(9.5))
                    .text_color(t.accent)
                    .child(tag),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .font_family("JetBrains Mono")
                    .text_size(px(12.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(shown),
            )
            .children((*kind == InputKind::Steer).then(|| {
                let mk = |label: &str, on: bool, q: bool, idx: u64| {
                    let root = root.clone();
                    div()
                        .id(("steer-mode", id.0 * 2 + idx))
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(6.))
                        .text_size(px(10.5))
                        .cursor_pointer()
                        .when(on, |d| d.bg(alpha(t.accent, 0.18)).text_color(t.accent))
                        .when(!on, |d| d.text_color(t.weak))
                        .child(label.to_string())
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| r.dispatch(DashAction::SetSteerQueue(q), cx));
                        })
                };
                div()
                    .flex()
                    .gap_0p5()
                    .child(mk("\u{1f3af} now", !queue, false, 0))
                    .child(mk("\u{1f4e8} queue", *queue, true, 1))
            }))
            .child(
                widgets::primary_btn(&t, send_label)
                    .id(("card-input-send", id.0))
                    .on_click({
                        let root = self.root.clone();
                        move |_, _, cx| {
                            root.update(cx, |r, cx| r.dispatch(DashAction::SubmitInput, cx));
                        }
                    }),
            );
        row.into_any_element()
    }

    /// State-dependent actions left, Changes + Open right.
    fn action_bar(&self) -> impl IntoElement {
        let t = self.t;
        let s = &self.snap;
        let id = s.id;
        let mut left: Vec<AnyElement> = Vec::new();
        let steer_open = matches!(self.inline, Some((InputKind::Steer, ..)));
        let send_open = matches!(self.inline, Some((InputKind::Send, ..)));

        match s.status {
            InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => {
                left.push(self.act_btn("\u{23f8} Pause", "act-pause", DashAction::Pause(id)));
                left.push(self.act_btn("\u{23f9} Stop", "act-stop", DashAction::Stop(id)));
                left.push(self.toggle_btn(
                    "\u{1f3af} Steer",
                    "act-steer",
                    steer_open,
                    InputKind::Steer,
                ));
            }
            InstanceStatus::Paused => {
                left.push(
                    widgets::primary_btn(&t, "\u{25b6} Resume")
                        .id(("act-resume", id.0))
                        .on_click(self.on(DashAction::Resume(id)))
                        .into_any_element(),
                );
                left.push(self.act_btn("\u{23f9} Stop", "act-stop", DashAction::Stop(id)));
                left.push(self.toggle_btn(
                    "\u{1f3af} Steer",
                    "act-steer",
                    steer_open,
                    InputKind::Steer,
                ));
            }
            InstanceStatus::WaitingForInput => {
                left.push(
                    widgets::primary_btn(&t, "Answer \u{2192}")
                        .id(("act-answer", id.0))
                        .on_click(self.on(DashAction::Open(id)))
                        .into_any_element(),
                );
            }
            InstanceStatus::Idle => {
                left.push(self.toggle_btn(
                    "\u{2709} New prompt",
                    "act-send",
                    send_open,
                    InputKind::Send,
                ));
            }
            InstanceStatus::Dead => {
                left.push(self.act_btn("\u{21bb} Restart", "act-restart", DashAction::Restart(id)));
            }
            InstanceStatus::Starting => {
                left.push(
                    div()
                        .text_size(px(11.))
                        .text_color(t.weak)
                        .child("warming up\u{2026}")
                        .into_any_element(),
                );
            }
        }

        let changes_label = if s.diff_count > 0 {
            format!("Changes ({})", s.diff_count)
        } else {
            "Changes".to_string()
        };
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .children(left)
            .child(div().flex_1())
            .child(self.act_btn(&changes_label, "act-changes", DashAction::Changes(id)))
            .child(self.act_btn("Open \u{2192}", "act-open", DashAction::Open(id)))
    }

    fn act_btn(&self, label: &str, key: &'static str, action: DashAction) -> AnyElement {
        let root = self.root.clone();
        widgets::btn(&self.t, label)
            .id((key, self.snap.id.0))
            .on_click(move |_, _, cx| {
                let a = action.clone();
                root.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .into_any_element()
    }

    fn toggle_btn(
        &self,
        label: &str,
        key: &'static str,
        open: bool,
        kind: InputKind,
    ) -> AnyElement {
        let t = self.t;
        let root = self.root.clone();
        let id = self.snap.id;
        widgets::btn(&t, label)
            .when(open, |d| {
                d.border_color(alpha(t.accent, 0.8)).text_color(t.accent)
            })
            .id((key, id.0))
            .on_click(move |_, window, cx| {
                root.update(cx, |r, cx| r.toggle_input(id, kind, window, cx));
            })
            .into_any_element()
    }
}

impl RenderOnce for AgentCard {
    fn render(self, _window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let t = self.t;
        let s = &self.snap;
        let border = if s.neutral {
            t.line_soft
        } else {
            alpha(s.color, 0.35)
        };
        let card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(t.card)
            .rounded(px(13.))
            .border_1()
            .border_color(border)
            .when(s.live, |d| d.shadow_md())
            .child(self.header())
            .child(self.state_line())
            .child(self.last_prompt())
            .child(self.context_bar())
            .child(self.stats_row())
            .child(self.sub_agents())
            .child(self.inline_input())
            .child(self.action_bar());

        // Entrance fade for fresh cards — one-shot per element id, gated on
        // reduce-motion. (The animation state lives with the element id, so
        // it only ever plays when the card first appears.)
        if self.reduce_motion {
            card.into_any_element()
        } else {
            card.with_animation(
                ("card-in", s.id.0),
                Animation::new(Duration::from_millis(280)).with_easing(ease_in_out),
                |el, delta| el.opacity(0.15 + 0.85 * delta),
            )
            .into_any_element()
        }
    }
}

/// 38px rounded avatar with the role emoji; live cards pulse an accent ring.
fn avatar(t: &Tokens, emoji: &str, live: bool, id: u64, reduce_motion: bool) -> AnyElement {
    let accent = t.accent;
    let base = div()
        .size(px(38.))
        .flex_none()
        .rounded(px(12.))
        .bg(t.well)
        .border_1()
        .border_color(if live {
            alpha(t.accent, 0.55)
        } else {
            t.line_soft
        })
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(18.))
        .child(emoji.to_string());
    if !live || reduce_motion {
        return base.into_any_element();
    }
    base.with_animation(
        ("avatar-ring", id),
        Animation::new(Duration::from_millis(3400))
            .repeat()
            .with_easing(ease_in_out),
        move |el, delta| {
            let tt = 1.0 - (delta * 2.0 - 1.0).abs();
            el.border_color(alpha(accent, 0.35 + 0.6 * tt))
        },
    )
    .into_any_element()
}
