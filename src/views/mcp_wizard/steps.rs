//! Per-step renderers + footers for the MCP "Add server" wizard, split out of
//! `mod.rs` to stay under the line budget. As a child module these read the
//! parent's private `Wizard`/`Transport` internals directly.

use eframe::egui;

use super::{Transport, Wizard, WizardAction, build_config, validate_fields};
use crate::views::common::paste_editor;

/// Paste-mode footer: Save validates + submits; Format syntax-checks + tidies.
pub(super) fn paste_footer(ui: &mut egui::Ui, wizard: &mut Wizard, action: &mut WizardAction) {
    if ui.button("Add server").clicked() {
        match wizard.apply_paste() {
            Ok(()) => *action = WizardAction::Save,
            Err(e) => wizard.error = Some(e),
        }
    }
    if ui
        .button("Format")
        .on_hover_text("Validate the JSON and tidy it")
        .clicked()
    {
        match wizard.apply_paste() {
            Ok(()) => {
                wizard.sync_paste_from_form();
                wizard.error = None;
            }
            Err(e) => wizard.error = Some(e),
        }
    }
}

/// Form-mode Next/Back/Add navigation footer for the current step.
pub(super) fn form_footer(ui: &mut egui::Ui, wizard: &mut Wizard, action: &mut WizardAction) {
    match wizard.step {
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
                *action = WizardAction::Save;
            }
            if ui.button("Back").clicked() {
                wizard.step = 1;
                wizard.error = None;
            }
        }
    }
}

pub(super) fn step_transport(ui: &mut egui::Ui, wizard: &mut Wizard) {
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

pub(super) fn step_fields(ui: &mut egui::Ui, wizard: &mut Wizard) {
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

pub(super) fn step_review(ui: &mut egui::Ui, wizard: &Wizard) {
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

pub(super) fn step_paste(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.label("Paste a server entry, e.g. {\"my-server\": {\"command\": \"npx\", ...}}:");
    ui.add_space(4.0);
    paste_editor(ui, "mcp-paste", &mut wizard.paste);
    ui.add_space(2.0);
    ui.weak(
        "An outer mcpServers wrapper is unwrapped; the transport is read from \
         a \"type\" field or inferred from command/url. Format tidies; Add validates.",
    );
}
