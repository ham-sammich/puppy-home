//! Agent manager: dispatch + render (egui `views/agent_manager.rs` +
//! `views/agent_wizard/` at parity, dressed in the GPUI tokens).

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::RootView;
use crate::gpui_ui::managers::{F_B, F_C, F_D, F_E, F_F, F_NAME, F_TOOLF, MgrAction};
use crate::gpui_ui::managers_ui::{
    MgrArgs, act, center_hint, disabled_btn, filter_field, mono_block, small,
};
use crate::gpui_ui::widgets::{self, alpha};
use crate::views::agent_manager::{matches_filter, source_badge};
use crate::views::agent_wizard::{self, Scope};
use crate::views::common::EditMode;
use crate::workspace::Workspace;

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl RootView {
    pub(crate) fn dispatch_agents(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        let accent = self.tokens.accent;
        match action {
            MgrAction::AgentSelect(name) => {
                self.mgr_selected = Some(name.clone());
                self.agent_delete_confirm = None;
                self.with_ready_backend(|b| b.get_agent_config(&name));
            }
            MgrAction::AgentClone(name) => {
                self.with_ready_backend(|b| b.clone_agent_config(&name));
                self.toast(format!("Cloning {name}\u{2026}"), accent);
                self.mgr_last_request = None; // re-list will surface the clone
                self.mgr_upkeep();
            }
            MgrAction::AgentDelete(name) => self.agent_delete_confirm = Some(name),
            MgrAction::AgentDeleteCancel => self.agent_delete_confirm = None,
            MgrAction::AgentDeleteConfirm => {
                if let Some(name) = self.agent_delete_confirm.take() {
                    self.with_ready_backend(|b| b.delete_agent_config(&name));
                    self.toast(format!("Deleted {name}"), accent);
                    self.mgr_selected = None;
                    self.mgr_last_request = None;
                    self.mgr_upkeep();
                }
            }
            MgrAction::AgentWizardOpen(edit) => {
                self.ensure_mgr_inputs(cx);
                let w = if edit {
                    // Edit needs the fetched detail (render gates the button).
                    let Some(ws) = self.serving_ws() else { return };
                    match (&self.mgr_selected, &ws.agent_config_detail) {
                        (Some(sel), Some(d)) if d.name == *sel => agent_wizard::Wizard::edit(d),
                        _ => return,
                    }
                } else {
                    agent_wizard::Wizard::create()
                };
                self.seed(F_NAME, w.name.clone(), cx);
                self.seed(F_B, w.display_name.clone(), cx);
                self.seed(F_C, w.description.clone(), cx);
                self.seed(F_D, w.model.clone(), cx);
                self.seed(F_E, w.system_prompt.clone(), cx);
                self.seed(F_F, w.user_prompt.clone(), cx);
                self.seed(F_TOOLF, String::new(), cx);
                self.agent_wizard = Some(w);
            }
            MgrAction::AgentWizardCancel => self.agent_wizard = None,
            MgrAction::AgentMode(paste) => {
                self.read_agent_inputs(cx);
                let mut seed = None;
                if let Some(w) = &mut self.agent_wizard {
                    w.error = None;
                    w.mode = if paste {
                        EditMode::Paste
                    } else {
                        EditMode::Form
                    };
                    if paste {
                        w.sync_paste_from_form();
                        seed = Some(w.paste.clone());
                    }
                }
                if let Some(text) = seed {
                    self.seed_paste(text, cx);
                }
            }
            MgrAction::AgentStep(delta) => {
                self.read_agent_inputs(cx);
                if let Some(w) = &mut self.agent_wizard {
                    w.error = None;
                    // egui gates Basics -> Prompt behind name + description.
                    if w.step == 0
                        && delta > 0
                        && let Err(e) = agent_wizard::validate_basics(w)
                    {
                        w.error = Some(e);
                        return;
                    }
                    w.step = (w.step as i32 + delta).clamp(0, 3) as usize;
                }
            }
            MgrAction::AgentScope(project) => {
                if let Some(w) = &mut self.agent_wizard {
                    w.scope = if project { Scope::Project } else { Scope::User };
                }
            }
            MgrAction::AgentToggleTool(tool) => {
                if let Some(w) = &mut self.agent_wizard {
                    if let Some(i) = w.tools.iter().position(|t| *t == tool) {
                        w.tools.remove(i);
                    } else {
                        w.tools.push(tool);
                    }
                }
            }
            MgrAction::AgentToggleMcp(name) => {
                if let Some(w) = &mut self.agent_wizard {
                    if let Some(i) = w.mcp_servers.iter().position(|t| *t == name) {
                        w.mcp_servers.remove(i);
                    } else {
                        w.mcp_servers.push(name);
                    }
                }
            }
            MgrAction::AgentToolsAll => {
                let catalog = self
                    .serving_ws()
                    .map(|ws| ws.agent_tool_catalog.clone())
                    .unwrap_or_default();
                if let Some(w) = &mut self.agent_wizard {
                    w.tools = catalog;
                }
            }
            MgrAction::AgentToolsNone => {
                if let Some(w) = &mut self.agent_wizard {
                    w.tools.clear();
                }
            }
            MgrAction::AgentFormat => {
                let text = self.paste_text(cx);
                let mut tidy = None;
                if let Some(w) = &mut self.agent_wizard {
                    w.paste = text;
                    match w.apply_paste() {
                        Ok(()) => {
                            w.error = None;
                            w.sync_paste_from_form();
                            tidy = Some(w.paste.clone());
                        }
                        Err(e) => w.error = Some(e),
                    }
                }
                if let Some(tidy) = tidy {
                    self.seed_paste(tidy, cx);
                    if let Some(w) = self.agent_wizard.take() {
                        // parsed fields -> form inputs
                        self.seed(F_NAME, w.name.clone(), cx);
                        self.seed(F_B, w.display_name.clone(), cx);
                        self.seed(F_C, w.description.clone(), cx);
                        self.seed(F_D, w.model.clone(), cx);
                        self.seed(F_E, w.system_prompt.clone(), cx);
                        self.seed(F_F, w.user_prompt.clone(), cx);
                        self.agent_wizard = Some(w);
                    }
                }
            }
            MgrAction::AgentSubmit => {
                self.read_agent_inputs(cx);
                let paste_mode = self
                    .agent_wizard
                    .as_ref()
                    .is_some_and(|w| w.mode == EditMode::Paste);
                if paste_mode {
                    let text = self.paste_text(cx);
                    if let Some(w) = &mut self.agent_wizard {
                        w.paste = text;
                        if let Err(e) = w.apply_paste() {
                            w.error = Some(e);
                            return;
                        }
                    }
                }
                let Some(w) = &mut self.agent_wizard else {
                    return;
                };
                if let Err(e) = agent_wizard::validate_basics(w) {
                    w.error = Some(e);
                    return;
                }
                let draft = w.draft();
                self.with_ready_backend(|b| b.save_agent_config(&draft));
                self.toast(format!("Saved agent {}", draft.name), accent);
                // Show the saved agent (egui: re-list bump re-fetches detail).
                self.mgr_selected = Some(draft.name);
                self.agent_wizard = None;
                self.mgr_last_request = None;
                self.mgr_upkeep();
            }
            _ => {}
        }
    }

    fn read_agent_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        let name = self.mgr_input_text(F_NAME, cx);
        let display = self.mgr_input_text(F_B, cx);
        let desc = self.mgr_input_text(F_C, cx);
        let model = self.mgr_input_text(F_D, cx);
        let sys = self.mgr_input_text(F_E, cx);
        let user = self.mgr_input_text(F_F, cx);
        if let Some(w) = &mut self.agent_wizard {
            w.name = name;
            w.display_name = display;
            w.description = desc;
            w.model = model;
            w.system_prompt = sys;
            w.user_prompt = user;
        }
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub(crate) fn body(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    if let Some(w) = args.agent_wizard {
        return super::managers_agents_wizard::wizard_body(args, ws, w);
    }
    match &ws.agent_configs {
        None => center_hint(&t, &["Loading agents..."]),
        Some(agents) if agents.is_empty() => center_hint(
            &t,
            &[
                "No agents found.",
                "Use \"Create agent\" to build your first one.",
            ],
        ),
        Some(agents) => {
            let needle = args.filter.trim().to_lowercase();
            let visible: Vec<_> = agents
                .iter()
                .filter(|a| matches_filter(a, &needle))
                .collect();
            let list = div()
                .w(px(300.))
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(filter_field(args, "Filter agents..."))
                .child(if visible.is_empty() {
                    small(&t, "No agents match the filter.", t.dim).into_any_element()
                } else {
                    div()
                        .id("agent-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .children(
                            visible
                                .iter()
                                .enumerate()
                                .map(|(i, a)| agent_row(args, i, a)),
                        )
                        .into_any_element()
                });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .child(list)
                .child(detail_pane(args, ws, agents))
                .into_any_element()
        }
    }
}

/// One agent row: name (+active), source badge, description, tools/model meta.
fn agent_row(args: &MgrArgs, i: usize, a: &crate::backend::AgentConfigInfo) -> AnyElement {
    let t = args.t;
    let sel = args.selected == Some(a.name.as_str());
    let mut label = a.name.clone();
    if a.current {
        label.push_str("  (active)");
    }
    let hover = if a.path.is_empty() {
        "built-in agent".to_string()
    } else {
        a.path.clone()
    };
    let mut meta = format!("{} tool(s)", a.tool_count);
    if !a.model.is_empty() {
        meta.push_str(" - ");
        meta.push_str(&a.model);
    }
    div()
        .id(("agent-row", i as u64))
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .rounded(px(7.))
        .cursor_pointer()
        .when(sel, |d| d.bg(alpha(t.accent, 0.12)))
        .hover(|d| d.bg(t.well))
        .tooltip(widgets::text_tip(hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(label),
                )
                .child(small(&t, source_badge(&a.source).to_string(), t.dim)),
        )
        .children((!a.description.is_empty()).then(|| {
            div()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(t.weak)
                .child(a.description.clone())
        }))
        .child(small(&t, meta, t.dim))
        .on_click(act(&args.root, MgrAction::AgentSelect(a.name.clone())))
        .into_any_element()
}

/// Read-only detail: header + Edit/Clone/Delete (gated), then the JSON.
fn detail_pane(
    args: &MgrArgs,
    ws: &Workspace,
    agents: &[crate::backend::AgentConfigInfo],
) -> AnyElement {
    let t = args.t;
    let Some(selected) = args.selected else {
        return center_hint(&t, &["Select an agent to view its config."]);
    };
    let row = agents.iter().find(|a| a.name == selected);
    let editable = row.is_some_and(|r| r.editable);
    let is_current = row.is_some_and(|r| r.current);
    let detail_ready = ws
        .agent_config_detail
        .as_ref()
        .is_some_and(|d| d.name == selected);
    let deleting = args.agent_delete_confirm == Some(selected);

    let mut header = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(selected.to_string()),
        )
        .child(div().flex_1())
        // Clone is always available (built-ins become editable copies).
        .child(
            widgets::btn(&t, "Clone")
                .id("agent-clone")
                .on_click(act(&args.root, MgrAction::AgentClone(selected.to_string()))),
        );
    if editable {
        header = header.child(if detail_ready {
            widgets::btn(&t, "Edit")
                .id("agent-edit")
                .on_click(act(&args.root, MgrAction::AgentWizardOpen(true)))
                .into_any_element()
        } else {
            disabled_btn(&t, "Edit").into_any_element()
        });
        header = header.child(if is_current {
            // egui: "Switch agents before deleting the active one".
            div()
                .id("agent-delete-blocked")
                .child(disabled_btn(&t, "Delete"))
                .tooltip(widgets::text_tip(
                    "Switch agents before deleting the active one".into(),
                ))
                .into_any_element()
        } else {
            widgets::btn(&t, "Delete")
                .id("agent-delete")
                .on_click(act(
                    &args.root,
                    MgrAction::AgentDelete(selected.to_string()),
                ))
                .into_any_element()
        });
    }

    // Inline delete confirmation (egui's "Yes, delete / Cancel" row).
    let confirm = deleting.then(|| {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(small(&t, format!("Delete agent \"{selected}\"?"), t.paused))
            .child(
                widgets::btn(&t, "Yes, delete")
                    .border_color(alpha(t.error, 0.8))
                    .id("agent-delete-yes")
                    .on_click(act(&args.root, MgrAction::AgentDeleteConfirm)),
            )
            .child(
                widgets::btn(&t, "Cancel")
                    .id("agent-delete-no")
                    .on_click(act(&args.root, MgrAction::AgentDeleteCancel)),
            )
    });

    let mut col = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .children(confirm);
    if let Some(r) = row {
        col = col.child(small(&t, source_badge(&r.source).to_string(), t.dim));
        if !r.description.is_empty() {
            col = col.child(small(&t, r.description.clone(), t.weak));
        }
    }

    match &ws.agent_config_detail {
        Some(d) if d.name == selected => {
            if !d.editable {
                col = col.child(small(&t, "Read-only built-in - clone to edit", t.dim));
            }
            if !d.path.is_empty() {
                col = col.child(small(&t, d.path.clone(), t.dim));
            }
            let mut info = div().flex().flex_col().gap_1();
            if !d.model.is_empty() {
                info = info.child(small(&t, format!("Model: {}", d.model), t.text));
            }
            info = info
                .child(small(
                    &t,
                    format!(
                        "{} tool(s), {} MCP binding(s)",
                        d.tools.len(),
                        d.mcp_servers.len()
                    ),
                    t.text,
                ))
                .child(small(&t, "Config JSON", t.dim))
                .child(mono_block(&t, d.content.clone()));
            col.child(
                div()
                    .id("agent-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(info),
            )
            .into_any_element()
        }
        _ => col
            .child(center_hint(&t, &["Loading agent..."]))
            .into_any_element(),
    }
}
