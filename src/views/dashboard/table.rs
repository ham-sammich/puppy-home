//! The dashboard's dense List view: one table row per agent, with indented
//! sub-agent rows. Same data + actions as the cards, minimum chrome.

use eframe::egui::{self, RichText};

use crate::shell::ShellAction;
use crate::theme::Accents;
use crate::views::widgets;
use crate::workspace::Workspace;

use super::{card, card_state, role_emoji};

/// The dense List view: one row per agent (+ indented sub-agent rows).
pub(super) fn render(
    ui: &mut egui::Ui,
    fleet: &[&Workspace],
    a: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    let weak = ui.visuals().weak_text_color();
    egui::Grid::new("dashboard-list")
        .num_columns(8)
        .striped(true)
        .spacing([14.0, 8.0])
        .show(ui, |ui| {
            for h in [
                "",
                "Directory",
                "Agent / Model",
                "State",
                "Last prompt",
                "Activity",
                "Cost",
                "",
            ] {
                ui.label(RichText::new(h).strong().small());
            }
            ui.end_row();

            for ws in fleet {
                let st = card_state(ws.status, a, weak);
                widgets::status_dot(ui, st.color, st.live);

                ui.label(RichText::new(&ws.name).strong())
                    .on_hover_text(ws.root.to_string_lossy());

                ui.vertical(|ui| {
                    ui.label(if ws.agent.is_empty() {
                        "—"
                    } else {
                        &ws.agent
                    });
                    let model = if ws.model.is_empty() {
                        "—"
                    } else {
                        &ws.model
                    };
                    ui.label(RichText::new(model).monospace().weak().small());
                });

                let mut state = st.label.to_string();
                if let Some(tool) = &ws.current_tool {
                    state = format!("{state} · {tool}");
                }
                ui.label(RichText::new(state).color(st.color));

                let lp = if ws.last_prompt.is_empty() {
                    "—"
                } else {
                    &ws.last_prompt
                };
                ui.add(egui::Label::new(RichText::new(lp).small().weak()).truncate())
                    .on_hover_text(&ws.last_prompt);

                let now = std::time::Instant::now();
                let mut activity = match ws.turn_started {
                    Some(t0) => widgets::fmt_elapsed(now.saturating_duration_since(t0).as_secs()),
                    None => {
                        widgets::fmt_ago(now.saturating_duration_since(ws.last_activity).as_secs())
                    }
                };
                if ws.tool_calls > 0 {
                    activity = format!("{activity} · {} tools", ws.tool_calls);
                }
                if ws.token_rate > 0.5 {
                    activity = format!("{activity} · {:.0} t/s", ws.token_rate);
                }
                let resp = ui.label(RichText::new(activity).monospace().small().weak());
                if !ws.run_stats.is_empty() {
                    resp.on_hover_text(&ws.run_stats);
                }

                let cost = ws.cost.map_or("—".to_string(), |c| format!("${c:.2}"));
                ui.label(RichText::new(cost).monospace().small());

                ui.horizontal(|ui| {
                    if ui.button("Open").on_hover_text("Focus this chat").clicked() {
                        actions.push(ShellAction::FocusChat(ws.id));
                    }
                    let diffs = ws.diff_count();
                    let label = if diffs > 0 {
                        format!("Changes ({diffs})")
                    } else {
                        "Changes".to_string()
                    };
                    if ui.button(label).on_hover_text("File changes").clicked() {
                        actions.push(ShellAction::ShowChanges(ws.id));
                    }
                    if ui
                        .button("\u{2715}")
                        .on_hover_text("Close workspace")
                        .clicked()
                    {
                        actions.push(ShellAction::Close(ws.id));
                    }
                });
                ui.end_row();

                for sa in &ws.sub_agents {
                    let color = card::sub_status_color(&sa.status, a);
                    widgets::status_dot(ui, color, false);
                    ui.label(
                        RichText::new(format!(
                            "↳ {} {}",
                            role_emoji(&sa.agent_name),
                            sa.agent_name
                        ))
                        .small(),
                    );
                    ui.label(RichText::new(&sa.model_name).monospace().weak().small());
                    let mut s = sa.status.clone();
                    if let Some(tool) = &sa.current_tool {
                        s = format!("{s} · {tool}");
                    }
                    ui.label(RichText::new(s).color(color).small());
                    ui.label("");
                    let mut act = widgets::fmt_elapsed(sa.elapsed as u64);
                    if sa.tool_call_count > 0 {
                        act = format!("{act} · {}t", sa.tool_call_count);
                    }
                    if sa.token_count > 0 {
                        act = format!("{act} · {} tok", sa.token_count);
                    }
                    ui.label(RichText::new(act).monospace().weak().small());
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                }
            }
        });
}
