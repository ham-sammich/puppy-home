//! The global dashboard: every running Code Puppy instance and its state.

use std::time::Instant;

use eframe::egui;

use crate::browser::BrowserManager;
use crate::shell::ShellAction;
use crate::supervisor::Supervisor;
use crate::workspace::InstanceStatus;

pub fn render(
    ui: &mut egui::Ui,
    sup: &Supervisor,
    browser: &BrowserManager,
    actions: &mut Vec<ShellAction>,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("Instances");
        ui.label(egui::RichText::new(format!("({})", sup.len())).weak());
    });
    ui.separator();

    // Attention banner: which workspaces are blocked on your input.
    let waiting: Vec<_> = sup
        .iter()
        .filter(|w| w.status == InstanceStatus::WaitingForInput)
        .map(|w| (w.id, w.name.clone()))
        .collect();
    if !waiting.is_empty() {
        let attn = egui::Color32::from_rgb(215, 156, 220);
        egui::Frame::group(ui.style())
            .fill(egui::Color32::from_rgb(40, 28, 42))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        attn,
                        format!("⚠ {} workspace(s) waiting for your input:", waiting.len()),
                    );
                    for (id, name) in &waiting {
                        if ui.button(name).clicked() {
                            actions.push(ShellAction::FocusChat(*id));
                        }
                    }
                });
            });
        ui.add_space(4.0);
    }

    plugins_section(ui, browser);

    if sup.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.weak("No workspaces open.");
            ui.weak("Use “📁 Open Folder…” above to start a Code Puppy instance.");
        });
        return;
    }

    let now = Instant::now();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("dashboard-grid")
                .num_columns(6)
                .striped(true)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    for h in ["", "Workspace", "Agent / Model", "State", "Activity", ""] {
                        ui.label(egui::RichText::new(h).strong().small());
                    }
                    ui.end_row();

                    for ws in sup.iter() {
                        // status dot
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 5.0, ws.status.color());

                        // name + path tooltip
                        ui.label(egui::RichText::new(&ws.name).strong())
                            .on_hover_text(ws.root.to_string_lossy());

                        // agent / model
                        ui.vertical(|ui| {
                            ui.label(if ws.agent.is_empty() {
                                "—"
                            } else {
                                &ws.agent
                            });
                            ui.label(
                                egui::RichText::new(if ws.model.is_empty() {
                                    "—"
                                } else {
                                    &ws.model
                                })
                                .weak()
                                .small(),
                            );
                        });

                        // state (+ current tool, ⚠ when waiting on input)
                        let waiting = ws.status == InstanceStatus::WaitingForInput;
                        let mut state = ws.status.label().to_string();
                        if let Some(tool) = &ws.current_tool {
                            state = format!("{state} · {tool}");
                        }
                        if waiting {
                            state = format!("⚠ {state}");
                        }
                        let state_text = egui::RichText::new(state).color(ws.status.color());
                        ui.label(if waiting {
                            state_text.strong()
                        } else {
                            state_text
                        });

                        // activity: elapsed + tool count
                        let mut activity = match ws.turn_started {
                            Some(start) => {
                                format!("{}s", now.saturating_duration_since(start).as_secs())
                            }
                            None => format!(
                                "{} ago",
                                humanize(now.saturating_duration_since(ws.last_activity).as_secs())
                            ),
                        };
                        if ws.tool_calls > 0 {
                            activity = format!("{activity} · {} tools", ws.tool_calls);
                        }
                        if ws.token_rate > 0.5 {
                            activity = format!("{activity} · {:.0} t/s", ws.token_rate);
                        }
                        let activity_label = ui.label(egui::RichText::new(activity).weak());
                        if !ws.run_stats.is_empty() {
                            activity_label.on_hover_text(&ws.run_stats);
                        }

                        // actions
                        ui.horizontal(|ui| {
                            if ui.button("Open").on_hover_text("Focus this chat").clicked() {
                                actions.push(ShellAction::FocusChat(ws.id));
                            }
                            let diff_label = if ws.diff_count() > 0 {
                                format!("Changes ({})", ws.diff_count())
                            } else {
                                "Changes".to_string()
                            };
                            if ui
                                .button(diff_label)
                                .on_hover_text("File changes")
                                .clicked()
                            {
                                actions.push(ShellAction::ShowChanges(ws.id));
                            }
                            if ui.button("✕").on_hover_text("Close workspace").clicked() {
                                actions.push(ShellAction::Close(ws.id));
                            }
                        });
                        ui.end_row();

                        // Concurrent sub-agents Code Puppy spawned this turn (invoke_agent).
                        for sa in &ws.sub_agents {
                            let color = sub_status_color(&sa.status);

                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 3.5, color);

                            ui.label(egui::RichText::new(format!("↳ {}", sa.agent_name)).small());
                            ui.label(egui::RichText::new(&sa.model_name).weak().small());

                            let mut sstate = sa.status.clone();
                            if let Some(tool) = &sa.current_tool {
                                sstate = format!("{sstate} · {tool}");
                            }
                            ui.label(egui::RichText::new(sstate).color(color).small());

                            let mut sact = format!("{}s", sa.elapsed as u64);
                            if sa.tool_call_count > 0 {
                                sact = format!("{sact} · {} tools", sa.tool_call_count);
                            }
                            if sa.token_count > 0 {
                                sact = format!("{sact} · {} tok", sa.token_count);
                            }
                            ui.label(egui::RichText::new(sact).weak().small());

                            ui.label("");
                            ui.end_row();
                        }
                    }
                });
        });
}

/// The optional-plugins list: name, version, and runnable/incompatible status.
fn plugins_section(ui: &mut egui::Ui, browser: &BrowserManager) {
    egui::CollapsingHeader::new(format!("Plugins ({})", browser.plugins().len()))
        .default_open(true)
        .show(ui, |ui| {
            if browser.plugins().is_empty() {
                ui.weak("No plugins installed. Open the Browser tab to install one.");
                return;
            }
            for p in browser.plugins() {
                let (label, color) = if p.is_runnable() {
                    ("ready", egui::Color32::from_rgb(120, 200, 140))
                } else if !p.manifest.is_compatible() {
                    ("incompatible", ui.visuals().warn_fg_color)
                } else {
                    ("exe missing", egui::Color32::from_rgb(220, 120, 120))
                };
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                    ui.label(egui::RichText::new(&p.manifest.name).strong())
                        .on_hover_text(p.dir.display().to_string());
                    ui.label(
                        egui::RichText::new(format!("v{}", p.manifest.version))
                            .weak()
                            .small(),
                    );
                    ui.label(egui::RichText::new(label).color(color).small());
                });
            }
        });
    ui.add_space(4.0);
    ui.separator();
}

/// Color a sub-agent's status string for the dashboard dot/label.
fn sub_status_color(status: &str) -> egui::Color32 {
    match status {
        "running" | "thinking" | "tool_calling" | "starting" => {
            egui::Color32::from_rgb(120, 190, 255)
        }
        "completed" | "done" | "success" => egui::Color32::from_rgb(120, 200, 140),
        "error" | "failed" | "cancelled" => egui::Color32::from_rgb(220, 120, 120),
        _ => egui::Color32::GRAY,
    }
}

fn humanize(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}
