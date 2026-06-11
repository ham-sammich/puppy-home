//! MCP Manager: Code Puppy's MCP servers as a dockable tab.
//!
//! MCP config is global in Code Puppy, but all data must flow through a
//! workspace's sidecar channel (the load-bearing invariant): we pick the
//! first ready workspace, read the catalog it received, and send ops through
//! its backend handle. A hint is shown when no workspace is connected.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{Map, Value, json};

use crate::supervisor::Supervisor;
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
    wizard: Option<Wizard>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Sse,
    Http,
}

impl Transport {
    fn wire(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Sse => "sse",
            Transport::Http => "http",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Transport::Stdio => "Command (stdio)",
            Transport::Sse => "Remote URL (SSE)",
            Transport::Http => "Remote URL (HTTP)",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Transport::Stdio => "Run a local process; talk over stdin/stdout.",
            Transport::Sse => "Connect to a server-sent-events endpoint.",
            Transport::Http => "Connect to a streamable-HTTP endpoint.",
        }
    }
}

/// The guided "Add MCP server" wizard (3 steps: transport, fields, review).
struct Wizard {
    step: usize,
    transport: Transport,
    name: String,
    command: String,
    /// One argument per line.
    args: String,
    env: Vec<(String, String)>,
    url: String,
    headers: Vec<(String, String)>,
    error: Option<String>,
}

impl Wizard {
    fn new() -> Self {
        Wizard {
            step: 0,
            transport: Transport::Stdio,
            name: String::new(),
            command: String::new(),
            args: String::new(),
            env: Vec::new(),
            url: String::new(),
            headers: Vec::new(),
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Mirror Code Puppy's registry rule: alphanumeric plus `-`/`_`, non-empty.
fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a server name is required".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name must be alphanumeric (hyphens and underscores allowed)".into());
    }
    Ok(())
}

/// Split the args textarea: one argument per line, trimmed, empties dropped.
fn split_args(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Key/value rows -> JSON object, skipping rows with an empty key.
fn pairs_to_map(rows: &[(String, String)]) -> Map<String, Value> {
    rows.iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), Value::String(v.clone())))
        .collect()
}

/// Build the Code Puppy server config object from the wizard's fields.
fn build_config(w: &Wizard) -> Value {
    match w.transport {
        Transport::Stdio => {
            let mut obj = Map::new();
            obj.insert("command".into(), json!(w.command.trim()));
            let args = split_args(&w.args);
            if !args.is_empty() {
                obj.insert("args".into(), json!(args));
            }
            let env = pairs_to_map(&w.env);
            if !env.is_empty() {
                obj.insert("env".into(), Value::Object(env));
            }
            Value::Object(obj)
        }
        Transport::Sse | Transport::Http => {
            let mut obj = Map::new();
            obj.insert("url".into(), json!(w.url.trim()));
            let headers = pairs_to_map(&w.headers);
            if !headers.is_empty() {
                obj.insert("headers".into(), Value::Object(headers));
            }
            Value::Object(obj)
        }
    }
}

/// Validate the fields step; mirrors Code Puppy's registry validation so the
/// user hears about problems before the op crosses the wire.
fn validate_fields(w: &Wizard) -> Result<(), String> {
    validate_name(&w.name)?;
    match w.transport {
        Transport::Stdio => {
            if w.command.trim().is_empty() {
                return Err("a command is required for a stdio server".into());
            }
        }
        Transport::Sse | Transport::Http => {
            let url = w.url.trim();
            if url.is_empty() {
                return Err("a URL is required".into());
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err("URL must start with http:// or https://".into());
            }
        }
    }
    Ok(())
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

/// A small animated on/off switch (the canonical egui toggle widget).
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = ui.spacing().interact_size.y * egui::vec2(2.0, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        ui.painter().rect_filled(rect, radius, visuals.bg_fill);
        let circle_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(circle_x, rect.center().y);
        ui.painter().circle(
            center,
            0.75 * radius,
            visuals.fg_stroke.color,
            visuals.fg_stroke,
        );
    }
    response
}

/// Pick the workspace that serves MCP data: the first ready one.
fn serving_workspace(sup: &Supervisor) -> Option<&Workspace> {
    sup.iter().find(|w| w.is_ready())
}

/// Render the MCP Manager tab.
pub fn render(ui: &mut egui::Ui, sup: &Supervisor, view: &mut McpManagerView) {
    let Some(ws) = serving_workspace(sup) else {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("No Code Puppy connected")
                        .heading()
                        .weak(),
                );
                ui.add_space(4.0);
                ui.weak(if sup.is_empty() {
                    "Open a folder to start a workspace - MCP data is read through its sidecar."
                } else {
                    "Waiting for a workspace's Code Puppy to become ready..."
                });
            });
        });
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
                view.wizard = Some(Wizard::new());
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

// ---------------------------------------------------------------------------
// Add-server wizard (modal)
// ---------------------------------------------------------------------------

fn render_wizard(ctx: &egui::Context, view: &mut McpManagerView, ws: &Workspace) {
    let Some(wizard) = &mut view.wizard else {
        return;
    };
    let mut close = false;
    let mut submit = false;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    egui::Window::new("Add MCP server")
        .id(egui::Id::new("mcp-add-wizard"))
        .collapsible(false)
        .resizable(false)
        // Anchor with hard margins (like the sessions modal) so the window
        // can never grow off-screen.
        .anchor(egui::Align2::LEFT_TOP, [16.0, 48.0])
        .show(ctx, |ui| {
            let screen = ui.ctx().content_rect();
            let w = (screen.width() - 32.0).clamp(320.0, 560.0);
            let h = (screen.height() - 96.0).clamp(240.0, 520.0);
            ui.set_min_width(w);
            ui.set_max_size(egui::vec2(w, h));

            let steps = ["Transport", "Details", "Review"];
            ui.horizontal(|ui| {
                for (i, label) in steps.iter().enumerate() {
                    let text = format!("{}. {label}", i + 1);
                    if i == wizard.step {
                        ui.strong(text);
                    } else {
                        ui.weak(text);
                    }
                    if i + 1 < steps.len() {
                        ui.weak(">");
                    }
                }
            });
            ui.separator();

            match wizard.step {
                0 => step_transport(ui, wizard),
                1 => step_fields(ui, wizard),
                _ => step_review(ui, wizard),
            }

            if let Some(err) = &wizard.error {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(220, 100, 100), err);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match wizard.step {
                        0 => {
                            if ui.button("Next").clicked() {
                                wizard.step = 1;
                                wizard.error = None;
                            }
                        }
                        1 => {
                            if ui.button("Next").clicked() {
                                match validate_fields(wizard) {
                                    Ok(()) => {
                                        wizard.step = 2;
                                        wizard.error = None;
                                    }
                                    Err(e) => wizard.error = Some(e),
                                }
                            }
                            if ui.button("Back").clicked() {
                                wizard.step = 0;
                                wizard.error = None;
                            }
                        }
                        _ => {
                            if ui.button("Add server").clicked() {
                                submit = true;
                            }
                            if ui.button("Back").clicked() {
                                wizard.step = 1;
                                wizard.error = None;
                            }
                        }
                    },
                );
            });
        });

    if submit
        && let Some(wizard) = &view.wizard
        && let Some(backend) = &ws.backend
    {
        let config = build_config(wizard);
        backend.add_mcp_server(wizard.name.trim(), wizard.transport.wire(), &config);
        view.last_request = None; // refresh promptly to show the new server
        close = true;
    }
    if close {
        view.wizard = None;
    }
}

fn step_transport(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.label("How does this MCP server run?");
    ui.add_space(4.0);
    for t in [Transport::Stdio, Transport::Sse, Transport::Http] {
        let selected = wizard.transport == t;
        if ui
            .selectable_label(selected, format!("{}\n    {}", t.label(), t.blurb()))
            .clicked()
        {
            wizard.transport = t;
        }
    }
}

fn step_fields(ui: &mut egui::Ui, wizard: &mut Wizard) {
    egui::Grid::new("mcp-wizard-fields")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("my-server (letters, digits, - and _)"),
            );
            ui.end_row();

            match wizard.transport {
                Transport::Stdio => {
                    ui.label("Command");
                    ui.add(
                        egui::TextEdit::singleline(&mut wizard.command)
                            .desired_width(f32::INFINITY)
                            .hint_text("npx"),
                    );
                    ui.end_row();

                    ui.label("Arguments");
                    ui.add(
                        egui::TextEdit::multiline(&mut wizard.args)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .hint_text("one argument per line"),
                    );
                    ui.end_row();
                }
                Transport::Sse | Transport::Http => {
                    ui.label("URL");
                    ui.add(
                        egui::TextEdit::singleline(&mut wizard.url)
                            .desired_width(f32::INFINITY)
                            .hint_text("https://example.com/mcp"),
                    );
                    ui.end_row();
                }
            }
        });

    ui.add_space(4.0);
    match wizard.transport {
        Transport::Stdio => kv_rows(
            ui,
            &mut wizard.env,
            "Environment variables",
            "NAME",
            "value",
        ),
        Transport::Sse | Transport::Http => {
            kv_rows(ui, &mut wizard.headers, "Headers", "Header", "value")
        }
    }
}

/// Editable key/value rows with add/remove buttons.
fn kv_rows(
    ui: &mut egui::Ui,
    rows: &mut Vec<(String, String)>,
    title: &str,
    key_hint: &str,
    val_hint: &str,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).small().weak());
        if ui.small_button("+").on_hover_text("Add a row").clicked() {
            rows.push((String::new(), String::new()));
        }
    });
    let mut remove: Option<usize> = None;
    for (i, (k, v)) in rows.iter_mut().enumerate() {
        ui.push_id(("kv", title, i), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(k)
                        .desired_width(140.0)
                        .hint_text(key_hint),
                );
                ui.add(
                    egui::TextEdit::singleline(v)
                        .desired_width(f32::INFINITY)
                        .hint_text(val_hint),
                );
                if ui.small_button("x").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        rows.remove(i);
    }
}

fn step_review(ui: &mut egui::Ui, wizard: &Wizard) {
    ui.label("Review and confirm:");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(wizard.name.trim());
        ui.label(egui::RichText::new(wizard.transport.wire()).weak());
    });
    let pretty = serde_json::to_string_pretty(&build_config(wizard)).unwrap_or_default();
    egui::ScrollArea::vertical()
        .max_height(200.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut pretty.as_str())
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
    ui.weak("The server is registered globally and enabled; toggle it off any time.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_wizard() -> Wizard {
        let mut w = Wizard::new();
        w.name = "fs".into();
        w.command = " npx ".into();
        w.args = "-y\n\n  server-filesystem  \n".into();
        w.env = vec![
            ("KEY".into(), "value".into()),
            ("".into(), "ignored".into()),
        ];
        w
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("my-server_2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("bad/slash").is_err());
    }

    #[test]
    fn args_split_one_per_line_trimmed() {
        assert_eq!(split_args("-y\n\n  server  \n"), vec!["-y", "server"]);
        assert!(split_args("").is_empty());
    }

    #[test]
    fn stdio_config_shape() {
        let cfg = build_config(&stdio_wizard());
        assert_eq!(
            cfg,
            json!({
                "command": "npx",
                "args": ["-y", "server-filesystem"],
                "env": {"KEY": "value"}
            })
        );
    }

    #[test]
    fn stdio_config_omits_empty_optionals() {
        let mut w = Wizard::new();
        w.name = "fs".into();
        w.command = "uvx".into();
        assert_eq!(build_config(&w), json!({"command": "uvx"}));
    }

    #[test]
    fn url_config_shape() {
        let mut w = Wizard::new();
        w.transport = Transport::Sse;
        w.name = "remote".into();
        w.url = " https://example.com/sse ".into();
        w.headers = vec![("Authorization".into(), "Bearer x".into())];
        assert_eq!(
            build_config(&w),
            json!({
                "url": "https://example.com/sse",
                "headers": {"Authorization": "Bearer x"}
            })
        );
    }

    #[test]
    fn field_validation_per_transport() {
        let mut w = Wizard::new();
        w.name = "ok".into();
        assert!(validate_fields(&w).is_err()); // stdio without command

        w.command = "npx".into();
        assert!(validate_fields(&w).is_ok());

        w.transport = Transport::Http;
        assert!(validate_fields(&w).is_err()); // http without url
        w.url = "ftp://nope".into();
        assert!(validate_fields(&w).is_err()); // bad scheme
        w.url = "http://localhost:9000".into();
        assert!(validate_fields(&w).is_ok());
    }

    #[test]
    fn state_colors_distinguish_lifecycle() {
        assert_ne!(state_color("running"), state_color("stopped"));
        assert_ne!(state_color("error"), state_color("running"));
        assert_eq!(state_color("anything-else"), state_color("stopped"));
    }
}
