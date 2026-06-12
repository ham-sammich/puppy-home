//! Agent Manager: Code Puppy's agents (JSON configs + built-ins) as a
//! dockable tab.
//!
//! Same load-bearing invariant as the MCP/Skills managers: agents are global/
//! project Code Puppy config, but all data flows through a workspace's sidecar
//! channel. We pick the first ready workspace, read the catalog it received,
//! and send ops through its backend handle. The visual builder lives in
//! [`super::agent_wizard`]; built-in (Python) agents are read-only and can be
//! cloned into editable JSON copies.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::backend::AgentConfigInfo;
use crate::supervisor::Supervisor;
use crate::views::agent_wizard::{Wizard, WizardAction};
use crate::views::common::{no_workspace_hint, serving_workspace};
use crate::workspace::{Workspace, WorkspaceId};

/// Re-poll cadence while the tab is visible (agents change rarely).
const REFRESH_EVERY: Duration = Duration::from_secs(10);
/// Minimum gap between polls (avoids spamming while the first answer is due).
const REQUEST_GAP: Duration = Duration::from_secs(2);

/// State for the Agent Manager tab (one instance, owned by the app).
#[derive(Default)]
pub struct AgentManagerView {
    filter: String,
    /// The agent whose detail pane is open (by name).
    selected: Option<String>,
    /// Which workspace served us last, and the catalog generation we saw.
    seen: Option<(WorkspaceId, u64)>,
    last_request: Option<Instant>,
    /// An agent pending a delete confirmation (by name).
    confirm_delete: Option<String>,
    wizard: Option<Wizard>,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Case-insensitive name/description filter. `needle` must be lowercased.
pub(crate) fn matches_filter(agent: &AgentConfigInfo, needle: &str) -> bool {
    needle.is_empty()
        || agent.name.to_lowercase().contains(needle)
        || agent.display_name.to_lowercase().contains(needle)
        || agent.description.to_lowercase().contains(needle)
}

/// A short, human badge for where an agent lives.
pub(crate) fn source_badge(source: &str) -> &str {
    match source {
        "user" => "user",
        "project" => "project",
        "builtin" => "built-in",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the Agent Manager tab.
pub fn render(ui: &mut egui::Ui, sup: &Supervisor, view: &mut AgentManagerView) {
    let Some(ws) = serving_workspace(sup) else {
        no_workspace_hint(ui, sup, "agent data");
        return;
    };

    // Fresh data (or a different serving workspace) re-fetches the open detail.
    let generation = (ws.id, ws.agent_configs_generation);
    if view.seen != Some(generation) {
        if view.seen.map(|(id, _)| id) != Some(ws.id) {
            view.last_request = None; // new source: request immediately
        }
        view.seen = Some(generation);
        if let (Some(sel), Some(backend)) = (&view.selected, &ws.backend) {
            backend.get_agent_config(sel);
        }
    }

    // Poll: immediately when we have nothing, then on a slow cadence.
    let stale = match view.last_request {
        None => true,
        Some(at) => {
            let need = if ws.agent_configs.is_none() {
                REQUEST_GAP
            } else {
                REFRESH_EVERY
            };
            at.elapsed() >= need
        }
    };
    if stale && let Some(backend) = &ws.backend {
        backend.list_agent_configs();
        view.last_request = Some(Instant::now());
    }

    let have_catalog = ws.agent_configs.is_some();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("Agents");
        ui.label(
            egui::RichText::new(format!("JSON + built-in agents - via {}", ws.name))
                .weak()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(have_catalog, egui::Button::new("Create agent"))
                .on_hover_text("Open the visual builder")
                .clicked()
            {
                view.wizard = Some(Wizard::create());
            }
            if ui.small_button("Refresh").clicked()
                && let Some(backend) = &ws.backend
            {
                backend.list_agent_configs();
                view.last_request = Some(Instant::now());
            }
        });
    });
    ui.separator();

    match &ws.agent_configs {
        None => {
            ui.weak("Loading agents...");
        }
        Some(agents) if agents.is_empty() => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.weak("No agents found.");
                ui.weak("Use \"Create agent\" to build your first one.");
            });
        }
        Some(agents) => {
            let agents = agents.clone(); // detach from ws borrow for the loop
            two_pane(ui, view, ws, &agents);
        }
    }

    render_wizard(ui.ctx(), view, ws);
}

/// Searchable list on the left, read-only detail pane on the right.
fn two_pane(
    ui: &mut egui::Ui,
    view: &mut AgentManagerView,
    ws: &Workspace,
    agents: &[AgentConfigInfo],
) {
    egui::Panel::left("agents-list")
        .resizable(true)
        .default_size(300.0)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(&mut view.filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter agents..."),
            );
            ui.add_space(4.0);
            let needle = view.filter.trim().to_lowercase();
            let visible: Vec<&AgentConfigInfo> = agents
                .iter()
                .filter(|a| matches_filter(a, &needle))
                .collect();
            if visible.is_empty() {
                ui.weak("No agents match the filter.");
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for agent in visible {
                        agent_row(ui, view, ws, agent);
                    }
                });
        });
    egui::CentralPanel::default().show_inside(ui, |ui| {
        detail_pane(ui, view, ws, agents);
    });
}

/// One agent row: name (click to open), description, source/model badges.
fn agent_row(
    ui: &mut egui::Ui,
    view: &mut AgentManagerView,
    ws: &Workspace,
    agent: &AgentConfigInfo,
) {
    ui.push_id(("agent-row", &agent.name), |ui| {
        ui.horizontal(|ui| {
            let selected = view.selected.as_deref() == Some(agent.name.as_str());
            let mut label = agent.name.clone();
            if agent.current {
                label.push_str("  (active)");
            }
            let resp = ui
                .selectable_label(selected, egui::RichText::new(label).strong())
                .on_hover_text(if agent.path.is_empty() {
                    "built-in agent"
                } else {
                    &agent.path
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(source_badge(&agent.source))
                        .weak()
                        .small(),
                );
            });
            if resp.clicked() {
                view.selected = Some(agent.name.clone());
                view.confirm_delete = None;
                if let Some(backend) = &ws.backend {
                    backend.get_agent_config(&agent.name);
                }
            }
        });
        if !agent.description.is_empty() {
            ui.add(
                egui::Label::new(egui::RichText::new(&agent.description).weak().small()).truncate(),
            )
            .on_hover_text(&agent.description);
        }
        // Compact meta line: tool count + pinned model (when set).
        let mut meta = format!("{} tool(s)", agent.tool_count);
        if !agent.model.is_empty() {
            meta.push_str(" - ");
            meta.push_str(&agent.model);
        }
        ui.label(egui::RichText::new(meta).weak().small());
        ui.separator();
    });
}

/// Read-only detail: header + actions (Edit/Clone/Delete), then the JSON.
fn detail_pane(
    ui: &mut egui::Ui,
    view: &mut AgentManagerView,
    ws: &Workspace,
    agents: &[AgentConfigInfo],
) {
    let Some(selected) = view.selected.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("Select an agent to view its config.");
        });
        return;
    };

    let row = agents.iter().find(|a| a.name == selected);
    let detail_ready = ws
        .agent_config_detail
        .as_ref()
        .is_some_and(|d| d.name == selected);

    ui.horizontal(|ui| {
        ui.strong(&selected);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let editable = row.is_some_and(|r| r.editable);
            let is_current = row.is_some_and(|r| r.current);

            // Clone is always available (built-ins become editable copies).
            if ui
                .button("Clone")
                .on_hover_text("Copy into an editable user JSON agent")
                .clicked()
                && let Some(backend) = &ws.backend
            {
                backend.clone_agent_config(&selected);
                view.last_request = None; // re-list will surface the clone
            }

            if editable
                && ui
                    .add_enabled(detail_ready, egui::Button::new("Edit"))
                    .on_hover_text("Open this agent in the builder (save overwrites)")
                    .clicked()
                && let Some(detail) = &ws.agent_config_detail
            {
                view.wizard = Some(Wizard::edit(detail));
            }

            if editable {
                let del = egui::Button::new(egui::RichText::new("Delete"));
                if ui
                    .add_enabled(!is_current, del)
                    .on_hover_text(if is_current {
                        "Switch agents before deleting the active one"
                    } else {
                        "Delete this agent's JSON file"
                    })
                    .clicked()
                {
                    view.confirm_delete = Some(selected.clone());
                }
            }
        });
    });

    // Inline delete confirmation.
    if view.confirm_delete.as_deref() == Some(selected.as_str()) {
        ui.horizontal(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(220, 150, 100),
                format!("Delete agent \"{selected}\"?"),
            );
            if ui.button("Yes, delete").clicked() {
                if let Some(backend) = &ws.backend {
                    backend.delete_agent_config(&selected);
                }
                view.confirm_delete = None;
                view.selected = None;
                view.last_request = None;
            }
            if ui.button("Cancel").clicked() {
                view.confirm_delete = None;
            }
        });
    }

    if let Some(r) = row {
        ui.label(egui::RichText::new(source_badge(&r.source)).weak().small());
        if !r.description.is_empty() {
            ui.label(egui::RichText::new(&r.description).weak());
        }
    }
    if let Some(detail) = &ws.agent_config_detail
        && detail.name == selected
    {
        if !detail.editable {
            ui.label(
                egui::RichText::new("Read-only built-in - clone to edit")
                    .weak()
                    .small(),
            );
        }
        if !detail.path.is_empty() {
            ui.label(egui::RichText::new(&detail.path).weak().small())
                .on_hover_text("On-disk JSON path");
        }
    }
    ui.separator();

    match &ws.agent_config_detail {
        Some(detail) if detail.name == selected => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !detail.model.is_empty() {
                        ui.label(format!("Model: {}", detail.model));
                    }
                    ui.label(format!(
                        "{} tool(s), {} MCP binding(s)",
                        detail.tools.len(),
                        detail.mcp_servers.len()
                    ));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Config JSON").weak().small());
                    let mut content = detail.content.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut content)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });
        }
        _ => {
            ui.weak("Loading agent...");
        }
    }
}

/// Drive the create/edit builder and act on its outcome.
fn render_wizard(ctx: &egui::Context, view: &mut AgentManagerView, ws: &Workspace) {
    let Some(wizard) = &mut view.wizard else {
        return;
    };
    match crate::views::agent_wizard::show(
        ctx,
        wizard,
        &ws.agent_tool_catalog,
        &ws.agent_mcp_catalog,
    ) {
        WizardAction::KeepOpen => {}
        WizardAction::Cancel => view.wizard = None,
        WizardAction::Save => {
            if let Some(backend) = &ws.backend {
                let draft = wizard.draft();
                backend.save_agent_config(&draft);
                // Show the saved agent: the re-list bump re-fetches the detail.
                view.selected = Some(draft.name.clone());
                view.last_request = None;
            }
            view.wizard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, display: &str, description: &str) -> AgentConfigInfo {
        AgentConfigInfo {
            name: name.into(),
            display_name: display.into(),
            description: description.into(),
            model: String::new(),
            tool_count: 0,
            source: "user".into(),
            editable: true,
            path: String::new(),
            current: false,
        }
    }

    #[test]
    fn filter_matches_name_display_and_description() {
        let a = info("qa-kitten", "QA Kitten", "Writes tests");
        assert!(matches_filter(&a, ""));
        assert!(matches_filter(&a, "kitten"));
        assert!(matches_filter(&a, "qa")); // matches display name
        assert!(matches_filter(&a, "tests"));
        assert!(!matches_filter(&a, "docker"));
    }

    #[test]
    fn source_badges_are_human() {
        assert_eq!(source_badge("user"), "user");
        assert_eq!(source_badge("project"), "project");
        assert_eq!(source_badge("builtin"), "built-in");
        assert_eq!(source_badge("weird"), "weird");
    }
}
