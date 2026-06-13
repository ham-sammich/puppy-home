//! Manager overlay frame + shared render helpers. Per-kind bodies live in
//! `managers_mcp` / `managers_skills` / `managers_agents`; state + dispatch
//! in `managers` (and the per-kind modules' `impl RootView` blocks).

use std::collections::HashMap;

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Rgba, Styled as _, div,
    prelude::*, px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers::{F_FILTER, MgrAction, MgrKind};
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::Workspace;

pub struct MgrArgs<'a> {
    pub t: Tokens,
    pub kind: MgrKind,
    pub ws: Option<&'a Workspace>,
    pub root: Entity<RootView>,
    pub inputs: &'a [Entity<ChatInput>],
    pub paste_input: Option<&'a Entity<ChatInput>>,
    pub filter: String,
    pub tool_filter: String,
    pub selected: Option<&'a str>,
    /// Optimistic toggle overrides (name -> desired), cleared on fresh data.
    pub pending: &'a HashMap<String, bool>,
    pub mcp_wizard: Option<&'a crate::views::mcp_wizard::Wizard>,
    pub skills_wizard: Option<&'a crate::views::skills_wizard::Wizard>,
    pub agent_wizard: Option<&'a crate::views::agent_wizard::Wizard>,
    pub agent_delete_confirm: Option<&'a str>,
    /// Models manager: the extra_models.json editor is open (QW4).
    pub models_editor: bool,
    /// Config manager: parsed puppy.cfg entries + the row being edited (QW5).
    pub cfg_entries: &'a [(String, String)],
    pub cfg_edit_key: Option<&'a str>,
}

/// Click handler that funnels a manager action through the root dispatch.
pub(crate) fn act(
    root: &Entity<RootView>,
    a: MgrAction,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    let root = root.clone();
    move |_, _, cx| {
        let a = a.clone();
        root.update(cx, |r, cx| r.dispatch(DashAction::Mgr(a), cx));
    }
}

pub(crate) fn small(t: &Tokens, text: impl Into<String>, color: Rgba) -> gpui::Div {
    let _ = t;
    div()
        .text_size(px(10.5))
        .text_color(color)
        .child(text.into())
}

/// A labelled framed input from the pool.
pub(crate) fn field(
    t: &Tokens,
    label: &str,
    input: Option<&Entity<ChatInput>>,
    tall: bool,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(small(t, label, t.weak))
        .children(input.map(|i| {
            // flex-col so the input's flex_grow fills the box vertically: the
            // WHOLE padded box (and a tall field's empty area) is clickable to
            // focus, not just the one text line (#feedback: click-to-focus).
            div()
                .flex()
                .flex_col()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .text_size(px(11.5))
                .when(tall, |d| d.min_h(px(90.)))
                .child(i.clone())
        }))
        .into_any_element()
}

/// Monospace read-only block (review previews, config JSON).
pub(crate) fn mono_block(t: &Tokens, text: String) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .font_family("JetBrains Mono")
        .text_size(px(11.))
        .text_color(t.text)
        .child(text)
}

/// A small static on/off switch (the egui `toggle_switch`, sans animation —
/// decorative motion stays out of state controls). Caller adds id/click.
pub(crate) fn switch(t: &Tokens, on: bool) -> gpui::Div {
    div()
        .w(px(30.))
        .h(px(16.))
        .flex_none()
        .rounded_full()
        .p(px(2.))
        .bg(if on { alpha(t.accent, 0.85) } else { t.well })
        .border_1()
        .border_color(if on {
            alpha(t.accent, 0.9)
        } else {
            t.line_soft
        })
        .flex()
        .when(on, |d| d.justify_end())
        .child(
            div()
                .size(px(10.))
                .rounded_full()
                .bg(if on { t.accent_ink } else { t.dim }),
        )
}

/// The "1. A > 2. B > 3. C" step strip (current step accented).
pub(crate) fn step_strip(t: &Tokens, labels: &[&str], current: usize) -> gpui::Div {
    div().flex().items_center().gap_1().children(
        labels
            .iter()
            .enumerate()
            .flat_map(|(i, label)| {
                let mut out = vec![
                    small(
                        t,
                        format!("{}. {label}", i + 1),
                        if i == current { t.accent } else { t.dim },
                    )
                    .when(i == current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .into_any_element(),
                ];
                if i + 1 < labels.len() {
                    out.push(small(t, ">", t.dim).into_any_element());
                }
                out
            })
            .collect::<Vec<_>>(),
    )
}

/// The "[Form] [Paste]" segmented toggle.
pub(crate) fn mode_toggle(
    args: &MgrArgs,
    paste_on: bool,
    to_form: MgrAction,
    to_paste: MgrAction,
) -> gpui::Div {
    let t = args.t;
    div()
        .flex()
        .gap_1()
        .child(
            widgets::btn(&t, "Form")
                .when(!paste_on, |d| d.border_color(alpha(t.accent, 0.8)))
                .id("mgr-mode-form")
                .on_click(act(&args.root, to_form)),
        )
        .child(
            widgets::btn(&t, "Paste")
                .when(paste_on, |d| d.border_color(alpha(t.accent, 0.8)))
                .id("mgr-mode-paste")
                .on_click(act(&args.root, to_paste)),
        )
}

/// A selectable option card (transport choices, save scopes).
pub(crate) fn option_card(t: &Tokens, on: bool, label: &str, blurb: &str) -> gpui::Div {
    div()
        .px_2p5()
        .py_1p5()
        .rounded(px(9.))
        .bg(t.card)
        .border_1()
        .border_color(if on {
            alpha(t.accent, 0.7)
        } else {
            t.line_soft
        })
        .cursor_pointer()
        .hover(|d| d.border_color(alpha(t.accent, 0.5)))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(t.text)
                .child(label.to_string()),
        )
        .child(small(t, blurb.to_string(), t.weak))
}

/// The bounded code-mode paste editor panel.
pub(crate) fn paste_panel(args: &MgrArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex_1()
        .min_h_0()
        .id("mgr-paste-scroll")
        .overflow_y_scroll()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .font_family("JetBrains Mono")
        .text_size(px(11.5))
        .children(args.paste_input.cloned())
        .into_any_element()
}

/// A visually disabled button shell (no click handler attached).
pub(crate) fn disabled_btn(t: &Tokens, label: &str) -> gpui::Div {
    div()
        .px_2p5()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .text_size(px(12.))
        .text_color(t.dim)
        .opacity(0.55)
        .child(label.to_string())
}

/// Centered weak hint (loading / empty states).
pub(crate) fn center_hint(t: &Tokens, lines: &[&str]) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .children(lines.iter().map(|l| {
            small(t, l.to_string(), t.dim)
                .text_size(px(12.))
                .into_any_element()
        }))
        .into_any_element()
}

/// The list filter input (pool slot `F_FILTER`).
pub(crate) fn filter_field(args: &MgrArgs, hint: &str) -> AnyElement {
    field(&args.t, hint, args.inputs.get(F_FILTER), false)
}

/// The centered overlay frame; body depends on the open manager.
pub fn overlay(args: &MgrArgs) -> AnyElement {
    let t = args.t;
    let wizard_open = match args.kind {
        MgrKind::Mcp => args.mcp_wizard.is_some(),
        MgrKind::Skills => args.skills_wizard.is_some(),
        MgrKind::Agents => args.agent_wizard.is_some(),
        MgrKind::Models => args.models_editor,
        MgrKind::Config => false,
    };
    let body: AnyElement = if args.kind == MgrKind::Config {
        // Config is file-based — no sidecar needed.
        super::managers_config::body(args)
    } else {
        match args.ws {
            None => center_hint(
                &t,
                &[
                    "No Code Puppy connected",
                    "Open a workspace first \u{2014} managers talk through its sidecar.",
                ],
            ),
            Some(ws) => match args.kind {
                MgrKind::Mcp => super::managers_mcp::body(args, ws),
                MgrKind::Skills => super::managers_skills::body(args, ws),
                MgrKind::Agents => super::managers_agents::body(args, ws),
                MgrKind::Models => super::managers_models::body(args, ws),
                MgrKind::Config => unreachable!("handled above"),
            },
        }
    };

    // Header: title + "subtitle - via {ws}" + Add/Create + Refresh + Close.
    let subtitle = args
        .ws
        .map(|ws| format!("{} \u{2014} via {}", args.kind.subtitle(), ws.name))
        .unwrap_or_else(|| args.kind.subtitle().to_string());
    let add_btn: Option<AnyElement> = if wizard_open || args.ws.is_none() {
        None
    } else {
        Some(match args.kind {
            MgrKind::Mcp => widgets::primary_btn(&t, "\u{ff0b} Add MCP server")
                .id("mgr-add")
                .on_click(act(&args.root, MgrAction::McpWizardOpen))
                .into_any_element(),
            MgrKind::Skills => widgets::primary_btn(&t, "\u{ff0b} Create skill")
                .id("mgr-add")
                .on_click(act(&args.root, MgrAction::SkillWizardOpen(false)))
                .into_any_element(),
            MgrKind::Agents => {
                // egui gates "Create agent" until the catalog has loaded.
                let have_catalog = args.ws.is_some_and(|ws| ws.agent_configs.is_some());
                let create: AnyElement = if have_catalog {
                    widgets::primary_btn(&t, "\u{ff0b} Create agent")
                        .id("mgr-add")
                        .on_click(act(&args.root, MgrAction::AgentWizardOpen(false)))
                        .into_any_element()
                } else {
                    disabled_btn(&t, "\u{ff0b} Create agent").into_any_element()
                };
                // QW7: conversational route — a fresh session driven by
                // code_puppy's built-in agent-creator agent.
                div()
                    .flex()
                    .gap_1p5()
                    .child(
                        widgets::btn(&t, "\u{1fa84} Create with Agent Creator")
                            .id("mgr-agent-creator")
                            .on_click(act(&args.root, MgrAction::AgentCreatorOpen)),
                    )
                    .child(create)
                    .into_any_element()
            }
            MgrKind::Models => widgets::primary_btn(&t, "\u{270e} Edit extra_models.json")
                .id("mgr-add")
                .on_click(act(&args.root, MgrAction::ModelsEditorOpen))
                .into_any_element(),
            // Config rows edit inline; no header action.
            MgrKind::Config => div().into_any_element(),
        })
    };

    let panel = div()
        .occlude()
        .w(px(760.))
        .max_w_full()
        .h(px(540.))
        // Height guard (mirrors max_w_full): on a short window the fixed 540px
        // panel used to overflow off-screen, hiding the footer/Save with no way
        // to scroll. Cap to the viewport so the body's scroll region engages.
        .max_h_full()
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
                        .child(args.kind.title()),
                )
                .child(small(&t, subtitle, t.weak))
                .child(div().flex_1())
                .children(add_btn)
                .child(
                    widgets::btn(&t, "\u{27f3} Refresh")
                        .id("mgr-refresh")
                        .on_click(act(&args.root, MgrAction::Refresh)),
                )
                .child(
                    widgets::btn(&t, "Close")
                        .id("mgr-close")
                        .on_click(act(&args.root, MgrAction::Close)),
                ),
        )
        .child(body);
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
    .with_priority(210)
    .into_any_element()
}
