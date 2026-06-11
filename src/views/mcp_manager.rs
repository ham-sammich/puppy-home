//! MCP Manager: Code Puppy's MCP servers as a dockable tab.
//!
//! MCP config is global in Code Puppy, but all data must flow through a
//! workspace's sidecar channel (the load-bearing invariant): we pick the
//! first ready workspace, read the catalog it received, and send ops through
//! its backend handle. A hint is shown when no workspace is connected.
//!
//! The "Add MCP server" wizard (guided form + raw paste) lives in `mcp_wizard`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::supervisor::Supervisor;
use crate::views::common::{no_workspace_hint, serving_workspace, toggle_switch};
use crate::views::mcp_wizard::{self, WizardAction};
use crate::workspace::{Workspace, WorkspaceId};

/// Re-poll cadence while the tab is visible (server state settles async).
const REFRESH_EVERY: Duration = Duration::from_secs(5);
/// Minimum gap between polls (avoids spamming while the first answer is due).
const REQUEST_GAP: Duration = Duration::from_secs(2);

/// State for the MCP Manager tab (one instance, owned by the app).
#[derive(Default)]
pub struct McpManagerView {
    /// Optimistic toggle overrides (name -> desired), cleared on fresh data.
    pending: HashMap<String, bool>,
    /// Which workspace served us last, and the catalog generation we saw.
    seen: Option<(WorkspaceId, u64)>,
    last_request: Option<Instant>,
    wizard: Option<mcp_wizard::Wizard>,
}

/// Status-dot color for a server's lifecycle state.
fn state_color(state: &str) -> egui::Color32 {
    match state {
        "running" => egui::Color32::from_rgb(110, 200, 110),
        "starting" | "stopping" => egui::Color32::from_rgb(220, 190, 90),
        "error" => egui::Color32::from_rgb(220, 100, 100),
        "quarantined" => egui::Color32::from_rgb(230, 150, 80),
        _ => egui::Color32::from_rgb(120, 120, 120), // stopped / unknown
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the MCP Manager tab.
pub fn render(ui: &mut egui::Ui, sup: &Supervisor, view: &mut McpManagerView) {
    let Some(ws) = serving_workspace(sup) else {
        no_workspace_hint(ui, sup, "MCP data");
        return;
    };

    // Fresh data (or a different serving workspace) clears optimistic state.
    let generation = (ws.id, ws.mcp_generation);
    if view.seen != Some(generation) {
        if view.seen.map(|(id, _)| id) != Some(ws.id) {
            view.last_request = None; // new source: request immediately
        }
        view.seen = Some(generation);
        view.pending.clear();
    }

    // Poll: immediately when we have nothing, then on a slow cadence.
    let stale = match view.last_request {
        None => true,
        Some(at) => {
            let need = if ws.mcp_servers.is_none() {
                REQUEST_GAP
            } else {
                REFRESH_EVERY
            };
            at.elapsed() >= need
        }
    };
    if stale && let Some(backend) = &ws.backend {
        backend.list_mcp_servers();
        view.last_request = Some(Instant::now());
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("MCP Servers");
        ui.label(
            egui::RichText::new(format!("global Code Puppy config - via {}", ws.name))
                .weak()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Add MCP server").clicked() {
                view.wizard = Some(mcp_wizard::Wizard::new());
            }
            if ui.small_button("Refresh").clicked()
                && let Some(backend) = &ws.backend
            {
                backend.list_mcp_servers();
                view.last_request = Some(Instant::now());
            }
        });
    });
    ui.separator();

    match &ws.mcp_servers {
        None => {
            ui.weak("Loading MCP servers...");
        }
        Some(servers) if servers.is_empty() => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.weak("No MCP servers registered yet.");
                ui.weak("Use \"Add MCP server\" to connect your first one.");
            });
        }
        Some(servers) => {
            let servers = servers.clone(); // detach from ws borrow for the loop
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for server in &servers {
                        server_row(ui, view, ws, server);
                    }
                });
        }
    }

    render_wizard(ui.ctx(), view, ws);
}

/// One server row: status dot, name, transport, summary, on/off switch.
fn server_row(
    ui: &mut egui::Ui,
    view: &mut McpManagerView,
    ws: &Workspace,
    server: &crate::backend::McpServerInfo,
) {
    ui.push_id(("mcp-row", &server.name), |ui| {
        ui.horizontal(|ui| {
            // Status dot (with the state name on hover).
            let (rect, dot) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 5.0, state_color(&server.state));
            let mut hover = format!("{} - {}", server.name, server.state);
            if !server.error.is_empty() {
                hover.push_str(&format!("\n{}", server.error));
            }
            dot.on_hover_text(hover);

            ui.label(egui::RichText::new(&server.name).strong());
            ui.label(egui::RichText::new(&server.transport).weak().small());
            if !server.error.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(220, 100, 100), "!")
                    .on_hover_text(&server.error);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Optimistic value: the pending toggle wins until fresh data.
                let mut on = view
                    .pending
                    .get(&server.name)
                    .copied()
                    .unwrap_or(server.enabled);
                if toggle_switch(ui, &mut on)
                    .on_hover_text(if on {
                        "Stop this server"
                    } else {
                        "Start this server"
                    })
                    .changed()
                {
                    view.pending.insert(server.name.clone(), on);
                    if let Some(backend) = &ws.backend {
                        backend.set_mcp_enabled(&server.name, on);
                    }
                }
                if !server.summary.is_empty() {
                    let summary = egui::RichText::new(&server.summary).weak().small();
                    ui.add(egui::Label::new(summary).truncate())
                        .on_hover_text(&server.summary);
                }
            });
        });
        ui.separator();
    });
}

/// Drive the add-server wizard modal; on Save, register via the workspace's
/// backend and refresh promptly so the new server appears.
fn render_wizard(ctx: &egui::Context, view: &mut McpManagerView, ws: &Workspace) {
    let Some(wizard) = &mut view.wizard else {
        return;
    };
    match mcp_wizard::show(ctx, wizard) {
        WizardAction::Save => {
            let name = wizard.name();
            let transport = wizard.transport_wire();
            let config = wizard.config();
            if let Some(backend) = &ws.backend {
                backend.add_mcp_server(&name, transport, &config);
            }
            view.last_request = None; // refresh promptly to show the new server
            view.wizard = None;
        }
        WizardAction::Cancel => view.wizard = None,
        WizardAction::KeepOpen => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_colors_distinguish_lifecycle() {
        assert_ne!(state_color("running"), state_color("stopped"));
        assert_ne!(state_color("error"), state_color("running"));
        assert_eq!(state_color("anything-else"), state_color("stopped"));
    }
}
