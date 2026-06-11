//! Per-step renderers for the Agent wizard, split out of `mod.rs` to keep each
//! file under the line budget. As a child module these can read the parent's
//! private `Wizard` fields and `Scope` helpers directly.

use eframe::egui;

use super::{Scope, Wizard, compose_preview};
use crate::views::common::paste_editor;

pub(super) fn step_basics(ui: &mut egui::Ui, wizard: &mut Wizard) {
    egui::Grid::new("agent-wizard-basics")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("my-agent (letters, digits, - and _)"),
            );
            ui.end_row();

            ui.label("Display name");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.display_name)
                    .desired_width(f32::INFINITY)
                    .hint_text("optional - shown in the agent picker"),
            );
            ui.end_row();

            ui.label("Description");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.description)
                    .desired_width(f32::INFINITY)
                    .hint_text("one line: what is this agent for?"),
            );
            ui.end_row();

            ui.label("Model");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.model)
                    .desired_width(f32::INFINITY)
                    .hint_text("optional - blank uses the global model"),
            );
            ui.end_row();
        });

    ui.add_space(6.0);
    ui.label(egui::RichText::new("Where to save").small().weak());
    for scope in [Scope::User, Scope::Project] {
        let selected = wizard.scope == scope;
        if ui
            .selectable_label(
                selected,
                format!("{}\n    {}", scope.label(), scope.blurb()),
            )
            .clicked()
        {
            wizard.scope = scope;
        }
    }
    if wizard.editing {
        ui.add_space(4.0);
        ui.weak(
            "Saving writes <agents dir>/<name>.json - keep the same name and \
             scope to overwrite in place.",
        );
    }
}

pub(super) fn step_prompt(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.label("System prompt (the agent's instructions):");
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("agent-system-prompt")
        .auto_shrink([false, true])
        .max_height(280.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut wizard.system_prompt)
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
    ui.add_space(8.0);
    ui.label("User prompt (optional - a canned opening message):");
    ui.add_space(4.0);
    ui.add(
        egui::TextEdit::multiline(&mut wizard.user_prompt)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text("leave blank to omit"),
    );
}

pub(super) fn step_tools(
    ui: &mut egui::Ui,
    wizard: &mut Wizard,
    tool_catalog: &[String],
    mcp_catalog: &[String],
) {
    ui.horizontal(|ui| {
        ui.label(format!("Tools ({} selected):", wizard.tools.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("None").clicked() {
                wizard.tools.clear();
            }
            if ui.small_button("All").clicked() {
                wizard.tools = tool_catalog.to_vec();
            }
        });
    });
    ui.add(
        egui::TextEdit::singleline(&mut wizard.tool_filter)
            .desired_width(f32::INFINITY)
            .hint_text("Filter tools..."),
    );
    ui.add_space(2.0);
    let needle = wizard.tool_filter.trim().to_lowercase();
    if tool_catalog.is_empty() {
        ui.weak("No tools reported by this Code Puppy.");
    } else {
        egui::ScrollArea::vertical()
            .id_salt("agent-tools")
            .auto_shrink([false, true])
            .max_height(220.0)
            .show(ui, |ui| {
                for tool in tool_catalog {
                    if !needle.is_empty() && !tool.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let mut on = wizard.tools.iter().any(|t| t == tool);
                    if ui.checkbox(&mut on, tool).changed() {
                        toggle(&mut wizard.tools, tool, on);
                    }
                }
            });
    }

    ui.add_space(8.0);
    ui.label(format!(
        "MCP server bindings ({} selected):",
        wizard.mcp_servers.len()
    ));
    if mcp_catalog.is_empty() {
        ui.weak("No MCP servers registered - add some in the MCP tab first.");
    } else {
        for server in mcp_catalog {
            let mut on = wizard.mcp_servers.iter().any(|s| s == server);
            if ui.checkbox(&mut on, server).changed() {
                toggle(&mut wizard.mcp_servers, server, on);
            }
        }
    }
}

/// Add or remove `item` from `set` to match `on` (keeps selections unique).
pub(super) fn toggle(set: &mut Vec<String>, item: &str, on: bool) {
    if on {
        if !set.iter().any(|x| x == item) {
            set.push(item.to_string());
        }
    } else {
        set.retain(|x| x != item);
    }
}

pub(super) fn step_review(ui: &mut egui::Ui, wizard: &Wizard) {
    ui.label("Review and confirm:");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(wizard.name.trim());
        ui.label(egui::RichText::new(wizard.scope.wire()).weak());
        ui.label(
            egui::RichText::new(format!(
                "{} tool(s), {} MCP",
                wizard.tools.len(),
                wizard.mcp_servers.len()
            ))
            .weak()
            .small(),
        );
    });
    let preview = compose_preview(wizard);
    egui::ScrollArea::vertical()
        .id_salt("agent-review")
        .max_height(320.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview.as_str())
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
    ui.weak(if wizard.editing {
        "Saving overwrites the existing agent JSON."
    } else {
        "The agent is discovered immediately and appears in the picker."
    });
}

pub(super) fn step_paste(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.label("Paste a full agent config (the JSON that lands on disk):");
    ui.add_space(4.0);
    paste_editor(ui, "agent-paste", &mut wizard.paste);
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Where to save").small().weak());
    for scope in [Scope::User, Scope::Project] {
        if ui
            .selectable_label(wizard.scope == scope, scope.label())
            .clicked()
        {
            wizard.scope = scope;
        }
    }
    ui.add_space(2.0);
    ui.weak("Format checks the JSON and tidies it; Save validates and writes it.");
}
