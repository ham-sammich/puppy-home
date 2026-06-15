//! The List view: one dense row per agent (directory, agent, model, state
//! badge, last prompt, elapsed, tok/s, cost, quick actions). Mirrors
//! `AgentRow` from `pack-card.jsx`.

use gpui::{
    AnyElement, Entity, FocusHandle, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    Styled as _, Window, div, prelude::*, px,
};

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::{InstanceStatus, WorkspaceId};

use super::{CardSnapshot, InputKind};

/// Column widths (px) for the fixed columns; Directory + Last prompt flex.
const W_AGENT: f32 = 92.0;
const W_MODEL: f32 = 170.0;
const W_STATE: f32 = 110.0;
const W_CLOCK: f32 = 64.0;
const W_TPS: f32 = 52.0;
const W_COST: f32 = 56.0;
const W_ACTIONS: f32 = 184.0;

#[derive(IntoElement)]
pub struct FleetTable {
    pub t: Tokens,
    pub rows: Vec<CardSnapshot>,
    pub root: Entity<RootView>,
    /// The one open inline input (steer / new prompt), if it's for a row here.
    pub input: Option<(WorkspaceId, InputKind, String, bool)>,
    pub input_focus: FocusHandle,
    /// A row awaiting a busy-close confirmation.
    pub close_confirm: Option<WorkspaceId>,
}

impl RenderOnce for FleetTable {
    fn render(self, _: &mut Window, _: &mut gpui::App) -> impl IntoElement {
        let t = self.t;
        let head = |label: &str, w: Option<f32>| {
            let cell = div()
                .text_size(px(10.))
                .text_color(t.weak)
                .child(label.to_string());
            match w {
                Some(w) => cell.w(px(w)).flex_none(),
                None => cell.flex_1().min_w_0(),
            }
        };
        div()
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
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .bg(t.panel)
                    .child(head("Directory", None))
                    .child(head("Agent", Some(W_AGENT)))
                    .child(head("Model", Some(W_MODEL)))
                    .child(head("State", Some(W_STATE)))
                    .child(head("Last prompt", None))
                    .child(head("Elapsed", Some(W_CLOCK)))
                    .child(head("tok/s", Some(W_TPS)))
                    .child(head("Cost", Some(W_COST)))
                    .child(head("", Some(W_ACTIONS))),
            )
            .children(self.rows.into_iter().map(|snap| {
                let id = snap.id;
                let name = snap.name.clone();
                // The active inline input (if any) and busy-close confirm both
                // expand BELOW the dense row — the row itself stays one line.
                let inline = self
                    .input
                    .as_ref()
                    .filter(|(iid, ..)| *iid == id)
                    .map(|(_, kind, text, queue)| (*kind, text.clone(), *queue));
                let confirming = self.close_confirm == Some(id);
                let main = row(&t, snap, &self.root);
                let extra: Option<AnyElement> = if confirming {
                    Some(close_confirm_bar(&t, id, &name, &self.root))
                } else {
                    inline.map(|(kind, text, queue)| {
                        super::inline_input_row(
                            &t,
                            id,
                            kind,
                            &text,
                            queue,
                            &self.root,
                            &self.input_focus,
                        )
                    })
                };
                div().flex().flex_col().child(main).children(extra)
            }))
    }
}

/// Inline "<name> is still running" End/Keep bar shown under a List row when
/// closing a busy puppy (mirrors the grid card's header confirm).
fn close_confirm_bar(
    t: &Tokens,
    id: WorkspaceId,
    name: &str,
    root: &Entity<RootView>,
) -> AnyElement {
    let root_end = root.clone();
    let root_keep = root.clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .bg(alpha(t.error, 0.08))
        .border_t_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex_1()
                .text_size(px(11.5))
                .text_color(t.error)
                .child(format!("{name} is still running")),
        )
        .child(
            widgets::btn(t, "End")
                .id(("row-close-yes", id.0))
                .on_click(move |_, _, cx| {
                    root_end.update(cx, |r, cx| r.dispatch(DashAction::CloseWorkspace(id), cx));
                }),
        )
        .child(
            widgets::btn(t, "Keep")
                .id(("row-close-no", id.0))
                .on_click(move |_, _, cx| {
                    root_keep.update(cx, |r, cx| r.dispatch(DashAction::CancelCloseWorkspace, cx));
                }),
        )
        .into_any_element()
}

fn row(t: &Tokens, s: CardSnapshot, root: &Entity<RootView>) -> AnyElement {
    let id = s.id;
    // Drag-reorder (#5): rows are drag handles too, carrying the same
    // card-shaped ghost as the grid so List view reorders identically.
    let drag_tok = *t;
    let drag_emoji = s.emoji.to_string();
    let drag_name = s.name.clone();
    let drag_status = s.label.to_string();
    let drag_color = s.color;
    let drop_hi = alpha(t.accent, 0.18);
    let root_drop = root.clone();
    let mono = |txt: String, color| {
        div()
            .font_family("JetBrains Mono")
            .text_size(px(11.5))
            .text_color(color)
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .child(txt)
    };
    let badge = div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(alpha(s.color, 0.16))
        .child(div().size(px(7.)).rounded_full().bg(s.color))
        .child(div().text_size(px(11.)).text_color(s.color).child(s.label));

    // Quick actions: state-appropriate icon buttons + Open.
    let mut acts: Vec<AnyElement> = Vec::new();
    let icon = |label: &str, key: &'static str, action: DashAction| {
        let root = root.clone();
        widgets::btn(t, label)
            .id((key, id.0))
            .on_click(move |_, _, cx| {
                let a = action.clone();
                root.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .into_any_element()
    };
    // Inline input toggles need the window (focus), so they're separate.
    let toggle = |label: &str, key: &'static str, kind: InputKind| {
        let root = root.clone();
        widgets::btn(t, label)
            .id((key, id.0))
            .on_click(move |_, window, cx| {
                root.update(cx, |r, cx| r.toggle_input(id, kind, window, cx));
            })
            .into_any_element()
    };
    match s.status {
        InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => {
            acts.push(icon("\u{23f8}", "row-pause", DashAction::Pause(id)));
            acts.push(icon("\u{23f9}", "row-stop", DashAction::Stop(id)));
            acts.push(toggle("\u{1f3af}", "row-steer", InputKind::Steer));
        }
        InstanceStatus::Paused => {
            acts.push(icon("\u{25b6}", "row-resume", DashAction::Resume(id)));
            acts.push(icon("\u{23f9}", "row-stop", DashAction::Stop(id)));
        }
        InstanceStatus::Dead => {
            acts.push(icon("\u{21bb}", "row-restart", DashAction::Restart(id)));
        }
        InstanceStatus::Idle => {
            acts.push(toggle("\u{2709}", "row-send", InputKind::Send));
        }
        _ => {}
    }
    acts.push(icon("\u{2192}", "row-open", DashAction::Open(id)));
    // Close this workspace (resting closes now; busy arms the confirm bar).
    acts.push(icon(
        "\u{2715}",
        "row-close",
        DashAction::RequestCloseWorkspace(id),
    ));

    div()
        .id(("fleet-row", id.0))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(t.line_soft)
        .hover(|d| d.bg(t.panel))
        .on_drag(id, move |_dragged, _pos, _win, cx| {
            cx.new(|_| widgets::CardGhost {
                t: drag_tok,
                emoji: drag_emoji.clone(),
                name: drag_name.clone(),
                label: drag_status.clone(),
                color: drag_color,
            })
        })
        .drag_over::<WorkspaceId>(move |style, _, _, _| style.bg(drop_hi))
        .on_drop::<WorkspaceId>(move |dragged, _, cx| {
            let moved = *dragged;
            root_drop.update(cx, |r, cx| {
                r.dispatch(DashAction::ReorderWorkspace { moved, target: id }, cx)
            });
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(s.emoji.to_string())
                        .child(s.name.clone()),
                )
                .child(mono(s.path.clone(), t.dim).text_size(px(10.5))),
        )
        .child(
            div()
                .w(px(W_AGENT))
                .flex_none()
                .child(mono(s.agent.clone(), t.weak)),
        )
        .child(
            // Fixed column by table design — ellipsizes; full id on hover
            // (B13.3 redux).
            div()
                .id(("row-model", s.id.0))
                .w(px(W_MODEL))
                .flex_none()
                .tooltip(widgets::text_tip(s.model.clone()))
                .child(mono(s.model.clone(), t.text)),
        )
        .child(div().w(px(W_STATE)).flex_none().child(badge))
        .child(
            div().flex_1().min_w_0().child(
                div()
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(if s.last_prompt.is_empty() {
                        "\u{2014}".to_string()
                    } else {
                        s.last_prompt.clone()
                    }),
            ),
        )
        .child(
            div()
                .w(px(W_CLOCK))
                .flex_none()
                .child(mono(s.clock.clone(), t.weak)),
        )
        .child(div().w(px(W_TPS)).flex_none().child(mono(
            if s.live {
                format!("{:.0}", s.rate)
            } else {
                "\u{2014}".to_string()
            },
            t.text,
        )))
        .child(div().w(px(W_COST)).flex_none().child(mono(
            match s.cost {
                Some(c) if s.cost_estimated => format!("\u{2248}${c:.2}"),
                Some(c) => format!("${c:.2}"),
                None => "\u{2014}".to_string(),
            },
            t.text,
        )))
        .child(
            div()
                .w(px(W_ACTIONS))
                .flex_none()
                .flex()
                .justify_end()
                .gap_1()
                .children(acts),
        )
        .into_any_element()
}
