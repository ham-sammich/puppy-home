//! The agent create/edit wizard render (egui `views/agent_wizard/` at
//! parity): 4-step form (basics, prompt, tools, review) + raw JSON paste
//! mode. Dispatch lives in `managers_agents`.

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::Tokens;
use crate::gpui_ui::managers::{F_B, F_C, F_D, F_E, F_F, F_NAME, F_TOOLF, MgrAction};
use crate::gpui_ui::managers_ui::{
    MgrArgs, act, field, mode_toggle, mono_block, option_card, paste_panel, small, step_strip,
};
use crate::gpui_ui::widgets::{self, alpha};
use crate::views::agent_wizard::{self, Scope};
use crate::views::common::EditMode;
use crate::workspace::Workspace;

pub(crate) fn wizard_body(args: &MgrArgs, ws: &Workspace, w: &agent_wizard::Wizard) -> AnyElement {
    let t = args.t;
    let paste = w.mode == EditMode::Paste;

    let scope_cards = |id_base: &'static str| {
        div().flex().flex_col().gap_1().children(
            [(Scope::User, false), (Scope::Project, true)]
                .into_iter()
                .enumerate()
                .map(|(i, (scope, project))| {
                    option_card(&t, w.scope == scope, scope.label(), scope.blurb())
                        .id((id_base, i as u64))
                        .on_click(act(&args.root, MgrAction::AgentScope(project)))
                        .into_any_element()
                }),
        )
    };

    let body: AnyElement = if paste {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(small(
                &t,
                "Paste a full agent config (the JSON that lands on disk):",
                t.weak,
            ))
            .child(paste_panel(args))
            .child(small(&t, "Where to save", t.weak))
            .child(scope_cards("agent-paste-scope"))
            .child(small(
                &t,
                "Format checks the JSON and tidies it; Save validates and writes it.",
                t.dim,
            ))
            .into_any_element()
    } else {
        match w.step {
            0 => div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(field(
                    &t,
                    "name \u{2014} my-agent (letters, digits, - and _)",
                    args.inputs.get(F_NAME),
                    false,
                ))
                .child(field(
                    &t,
                    "display name \u{2014} optional, shown in the agent picker",
                    args.inputs.get(F_B),
                    false,
                ))
                .child(field(
                    &t,
                    "description \u{2014} one line: what is this agent for?",
                    args.inputs.get(F_C),
                    false,
                ))
                .child(field(
                    &t,
                    "model \u{2014} optional, blank uses the global model",
                    args.inputs.get(F_D),
                    false,
                ))
                .child(model_picker(args, ws, w))
                .child(small(&t, "Where to save", t.weak))
                .child(scope_cards("agent-scope"))
                .children(w.editing.then(|| {
                    small(
                        &t,
                        "Saving writes <agents dir>/<name>.json - keep the same name and \
                         scope to overwrite in place.",
                        t.dim,
                    )
                }))
                .into_any_element(),
            1 => div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(field(
                    &t,
                    "system prompt (the agent's instructions)",
                    args.inputs.get(F_E),
                    true,
                ))
                .child(field(
                    &t,
                    "user prompt (optional - a canned opening message)",
                    args.inputs.get(F_F),
                    true,
                ))
                .into_any_element(),
            2 => tools_step(args, ws, w),
            _ => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(small(&t, "Review and confirm:", t.weak))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(w.name.trim().to_string()),
                        )
                        .child(small(&t, w.scope.wire(), t.weak))
                        .child(small(
                            &t,
                            format!("{} tool(s), {} MCP", w.tools.len(), w.mcp_servers.len()),
                            t.dim,
                        )),
                )
                .child(mono_block(&t, agent_wizard::compose_preview(w)))
                .child(small(
                    &t,
                    if w.editing {
                        "Saving overwrites the existing agent JSON."
                    } else {
                        "The agent is discovered immediately and appears in the picker."
                    },
                    t.dim,
                ))
                .into_any_element(),
        }
    };

    let footer = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            widgets::btn(&t, "Cancel")
                .id("agent-cancel")
                .on_click(act(&args.root, MgrAction::AgentWizardCancel)),
        )
        .child(div().flex_1())
        .children(paste.then(|| {
            widgets::btn(&t, "Format")
                .id("agent-format")
                .on_click(act(&args.root, MgrAction::AgentFormat))
        }))
        .children((!paste && w.step > 0).then(|| {
            widgets::btn(&t, "\u{2190} Back")
                .id("agent-back")
                .on_click(act(&args.root, MgrAction::AgentStep(-1)))
        }))
        .children((!paste && w.step < 3).then(|| {
            widgets::btn(&t, "Next \u{2192}")
                .id("agent-next")
                .on_click(act(&args.root, MgrAction::AgentStep(1)))
        }))
        .children((paste || w.step == 3).then(|| {
            widgets::primary_btn(&t, "\u{2713} Save agent")
                .id("agent-save")
                .on_click(act(&args.root, MgrAction::AgentSubmit))
        }));

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(small(&t, w.title(), t.text))
                .children(
                    (!paste)
                        .then(|| step_strip(&t, &["Basics", "Prompt", "Tools", "Review"], w.step)),
                )
                .child(div().flex_1())
                .child(mode_toggle(
                    args,
                    paste,
                    MgrAction::AgentMode(false),
                    MgrAction::AgentMode(true),
                )),
        )
        .children(w.error.clone().map(|e| small(&t, e, t.error)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .id("agent-wiz-scroll")
                .overflow_y_scroll()
                .child(body),
        )
        .child(footer)
        .into_any_element()
}

/// A pick-list of this Code Puppy's announced models that seeds the free-text
/// model field. Empty when the sidecar reported no catalog (field stays free).
fn model_picker(args: &MgrArgs, ws: &Workspace, w: &agent_wizard::Wizard) -> AnyElement {
    let t = args.t;
    let catalog = ws.model_catalog();
    if catalog.is_empty() {
        return div().into_any_element();
    }
    let cur = w.model.trim().to_string();
    let chip = |id: u64, label: String, on: bool, model: String| {
        div()
            .id(("agent-model-chip", id))
            .px_2()
            .py_0p5()
            .rounded_full()
            .bg(if on { alpha(t.accent, 0.16) } else { t.well })
            .border_1()
            .border_color(if on {
                alpha(t.accent, 0.7)
            } else {
                t.line_soft
            })
            .font_family("JetBrains Mono")
            .text_size(px(11.))
            .text_color(if on { t.accent } else { t.weak })
            .cursor_pointer()
            .hover(|d| d.border_color(alpha(t.accent, 0.5)))
            .child(label)
            .on_click(act(&args.root, MgrAction::AgentSetModel(model)))
            .into_any_element()
    };
    let mut row = div().flex().flex_wrap().gap_1().child(chip(
        0,
        "global default".to_string(),
        cur.is_empty(),
        String::new(),
    ));
    for (i, m) in catalog.iter().enumerate() {
        let on = cur == m.name;
        row = row.child(chip(i as u64 + 1, m.name.clone(), on, m.name.clone()));
    }
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(small(&t, "or pick from this Code Puppy's models:", t.dim))
        .child(row)
        .into_any_element()
}

/// Tools step: filter + All/None over the tool catalog, then MCP bindings.
fn tools_step(args: &MgrArgs, ws: &Workspace, w: &agent_wizard::Wizard) -> AnyElement {
    let t = args.t;
    let needle = args.tool_filter.trim().to_lowercase();
    let mut col = div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(small(
                    &t,
                    format!("Tools ({} selected):", w.tools.len()),
                    t.text,
                ))
                .child(div().flex_1())
                .child(
                    widgets::btn(&t, "All")
                        .id("agent-tools-all")
                        .on_click(act(&args.root, MgrAction::AgentToolsAll)),
                )
                .child(
                    widgets::btn(&t, "None")
                        .id("agent-tools-none")
                        .on_click(act(&args.root, MgrAction::AgentToolsNone)),
                ),
        )
        .child(field(
            &t,
            "Filter tools...",
            args.inputs.get(F_TOOLF),
            false,
        ));
    if ws.agent_tool_catalog.is_empty() {
        col = col.child(small(&t, "No tools reported by this Code Puppy.", t.dim));
    } else {
        let visible: Vec<&String> = ws
            .agent_tool_catalog
            .iter()
            .filter(|tool| needle.is_empty() || tool.to_lowercase().contains(&needle))
            .collect();
        col = col.child(chip_grid(&t, args, &visible, &w.tools, true));
    }
    col = col.child(small(
        &t,
        format!("MCP server bindings ({} selected):", w.mcp_servers.len()),
        t.text,
    ));
    if ws.agent_mcp_catalog.is_empty() {
        col = col.child(small(
            &t,
            "No MCP servers registered - add some in the MCP tab first.",
            t.dim,
        ));
    } else {
        let all: Vec<&String> = ws.agent_mcp_catalog.iter().collect();
        col = col.child(chip_grid(&t, args, &all, &w.mcp_servers, false));
    }
    col.into_any_element()
}

/// Toggleable chips for the tool / MCP-binding catalogs.
fn chip_grid(
    t: &Tokens,
    args: &MgrArgs,
    catalog: &[&String],
    chosen: &[String],
    tools: bool,
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_1()
        .children(catalog.iter().enumerate().map(|(i, item)| {
            let on = chosen.contains(item);
            let action = if tools {
                MgrAction::AgentToggleTool((*item).clone())
            } else {
                MgrAction::AgentToggleMcp((*item).clone())
            };
            div()
                .id((if tools { "tool-chip" } else { "mcp-chip" }, i as u64))
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(if on { alpha(t.accent, 0.16) } else { t.well })
                .border_1()
                .border_color(if on {
                    alpha(t.accent, 0.7)
                } else {
                    t.line_soft
                })
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .text_color(if on { t.accent } else { t.weak })
                .cursor_pointer()
                .hover(|d| d.border_color(alpha(t.accent, 0.5)))
                .child((*item).clone())
                .on_click(act(&args.root, action))
                .into_any_element()
        }))
        .into_any_element()
}
