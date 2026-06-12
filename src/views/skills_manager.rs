//! Skills Manager: Code Puppy's skills (SKILL.md folders) as a dockable tab.
//!
//! Same load-bearing invariant as the MCP manager: skills are global/project
//! Code Puppy config, but all data flows through a workspace's sidecar
//! channel. We pick the first ready workspace, read the catalog it received,
//! and send ops through its backend handle. The create/edit modal lives in
//! [`super::skills_wizard`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::backend::SkillInfo;
use crate::supervisor::Supervisor;
use crate::views::common::{no_workspace_hint, serving_workspace, toggle_switch};
use crate::views::skills_wizard::{Wizard, WizardAction, scope_for_source};
use crate::workspace::{Workspace, WorkspaceId};

/// Re-poll cadence while the tab is visible (skills change rarely).
const REFRESH_EVERY: Duration = Duration::from_secs(10);
/// Minimum gap between polls (avoids spamming while the first answer is due).
const REQUEST_GAP: Duration = Duration::from_secs(2);

/// State for the Skills Manager tab (one instance, owned by the app).
#[derive(Default)]
pub struct SkillsManagerView {
    filter: String,
    /// The skill whose detail pane is open (by name).
    selected: Option<String>,
    /// Optimistic toggle overrides (name -> desired), cleared on fresh data.
    pending: HashMap<String, bool>,
    /// Which workspace served us last, and the catalog generation we saw.
    seen: Option<(WorkspaceId, u64)>,
    last_request: Option<Instant>,
    wizard: Option<Wizard>,
    md_cache: CommonMarkCache,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Return the markdown body of a SKILL.md: everything after the closing
/// `---` frontmatter fence. Content without a well-formed fence is returned
/// unchanged.
pub(crate) fn skill_body(content: &str) -> &str {
    let trimmed = content.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return content;
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return content;
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return rest[offset + line.len()..].trim_start_matches(['\r', '\n']);
        }
        offset += line.len();
    }
    content // no closing fence: treat the whole thing as body
}

/// Case-insensitive name/description filter. `needle` must be lowercased.
pub(crate) fn matches_filter(skill: &SkillInfo, needle: &str) -> bool {
    needle.is_empty()
        || skill.name.to_lowercase().contains(needle)
        || skill.description.to_lowercase().contains(needle)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the Skills Manager tab.
pub fn render(ui: &mut egui::Ui, sup: &Supervisor, view: &mut SkillsManagerView) {
    let Some(ws) = serving_workspace(sup) else {
        no_workspace_hint(ui, sup, "skill data");
        return;
    };

    // Fresh data (or a different serving workspace) clears optimistic state
    // and re-fetches the open detail (its content may have just changed).
    let generation = (ws.id, ws.skills_generation);
    if view.seen != Some(generation) {
        if view.seen.map(|(id, _)| id) != Some(ws.id) {
            view.last_request = None; // new source: request immediately
        }
        view.seen = Some(generation);
        view.pending.clear();
        if let (Some(sel), Some(backend)) = (&view.selected, &ws.backend) {
            backend.get_skill(sel);
        }
    }

    // Poll: immediately when we have nothing, then on a slow cadence.
    let stale = match view.last_request {
        None => true,
        Some(at) => {
            let need = if ws.skills.is_none() {
                REQUEST_GAP
            } else {
                REFRESH_EVERY
            };
            at.elapsed() >= need
        }
    };
    if stale && let Some(backend) = &ws.backend {
        backend.list_skills();
        view.last_request = Some(Instant::now());
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading("Skills");
        ui.label(
            egui::RichText::new(format!("user + project skills - via {}", ws.name))
                .weak()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Create skill").clicked() {
                view.wizard = Some(Wizard::create());
            }
            if ui.small_button("Refresh").clicked()
                && let Some(backend) = &ws.backend
            {
                backend.list_skills();
                view.last_request = Some(Instant::now());
            }
        });
    });
    ui.separator();

    match &ws.skills {
        None => {
            ui.weak("Loading skills...");
        }
        Some(skills) if skills.is_empty() => {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.weak("No skills found.");
                ui.weak("Use \"Create skill\" to write your first SKILL.md.");
            });
        }
        Some(skills) => {
            let skills = skills.clone(); // detach from ws borrow for the loop
            two_pane(ui, view, ws, &skills);
        }
    }

    render_wizard(ui.ctx(), view, ws);
}

/// Searchable list on the left, read-only detail pane on the right.
fn two_pane(ui: &mut egui::Ui, view: &mut SkillsManagerView, ws: &Workspace, skills: &[SkillInfo]) {
    egui::Panel::left("skills-list")
        .resizable(true)
        .default_size(280.0)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(&mut view.filter)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter skills..."),
            );
            ui.add_space(4.0);
            let needle = view.filter.trim().to_lowercase();
            let visible: Vec<&SkillInfo> = skills
                .iter()
                .filter(|s| matches_filter(s, &needle))
                .collect();
            if visible.is_empty() {
                ui.weak("No skills match the filter.");
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for skill in visible {
                        skill_row(ui, view, ws, skill);
                    }
                });
        });
    egui::CentralPanel::default().show_inside(ui, |ui| {
        detail_pane(ui, view, ws);
    });
}

/// One skill row: name (click to open), description, source, on/off switch.
fn skill_row(ui: &mut egui::Ui, view: &mut SkillsManagerView, ws: &Workspace, skill: &SkillInfo) {
    ui.push_id(("skill-row", &skill.name), |ui| {
        ui.horizontal(|ui| {
            let selected = view.selected.as_deref() == Some(skill.name.as_str());
            let label = ui
                .selectable_label(selected, egui::RichText::new(&skill.name).strong())
                .on_hover_text(&skill.path);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Optimistic value: the pending toggle wins until fresh data.
                let mut on = view
                    .pending
                    .get(&skill.name)
                    .copied()
                    .unwrap_or(skill.enabled);
                if toggle_switch(ui, &mut on)
                    .on_hover_text(if on {
                        "Disable this skill"
                    } else {
                        "Enable this skill"
                    })
                    .changed()
                {
                    view.pending.insert(skill.name.clone(), on);
                    if let Some(backend) = &ws.backend {
                        backend.set_skill_enabled(&skill.name, on);
                    }
                }
                ui.label(egui::RichText::new(&skill.source).weak().small());
            });
            if label.clicked() {
                view.selected = Some(skill.name.clone());
                if let Some(backend) = &ws.backend {
                    backend.get_skill(&skill.name);
                }
            }
        });
        if !skill.description.is_empty() {
            ui.add(
                egui::Label::new(egui::RichText::new(&skill.description).weak().small()).truncate(),
            )
            .on_hover_text(&skill.description);
        }
        ui.separator();
    });
}

/// Read-only detail: path + description header, then rendered markdown body.
fn detail_pane(ui: &mut egui::Ui, view: &mut SkillsManagerView, ws: &Workspace) {
    let Some(selected) = view.selected.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("Select a skill to view its SKILL.md.");
        });
        return;
    };

    ui.horizontal(|ui| {
        ui.strong(&selected);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let detail_ready = ws.skill_detail.as_ref().is_some_and(|d| d.name == selected);
            if ui
                .add_enabled(detail_ready, egui::Button::new("Edit"))
                .on_hover_text("Open this skill in the wizard (save overwrites)")
                .clicked()
                && let Some(detail) = &ws.skill_detail
            {
                let source = ws
                    .skills
                    .as_deref()
                    .and_then(|all| all.iter().find(|s| s.name == selected))
                    .map(|s| s.source.as_str())
                    .unwrap_or("user");
                view.wizard = Some(Wizard::edit(
                    &detail.name,
                    &detail.description,
                    skill_body(&detail.content),
                    scope_for_source(source),
                ));
            }
        });
    });

    match &ws.skill_detail {
        Some(detail) if detail.name == selected => {
            ui.label(egui::RichText::new(&detail.path).weak().small());
            if !detail.description.is_empty() {
                ui.label(egui::RichText::new(&detail.description).weak());
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    CommonMarkViewer::new().show(
                        ui,
                        &mut view.md_cache,
                        skill_body(&detail.content),
                    );
                });
        }
        _ => {
            ui.weak("Loading skill...");
        }
    }
}

/// Drive the create/edit modal and act on its outcome.
fn render_wizard(ctx: &egui::Context, view: &mut SkillsManagerView, ws: &Workspace) {
    let Some(wizard) = &mut view.wizard else {
        return;
    };
    match crate::views::skills_wizard::show(ctx, wizard) {
        WizardAction::KeepOpen => {}
        WizardAction::Cancel => view.wizard = None,
        WizardAction::Save => {
            if let Some(backend) = &ws.backend {
                backend.save_skill(
                    wizard.name.trim(),
                    wizard.description.trim(),
                    &wizard.content,
                    wizard.scope.wire(),
                );
                // Show the saved skill: the re-list bump re-fetches the detail.
                view.selected = Some(wizard.name.trim().to_string());
                view.last_request = None;
            }
            view.wizard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, description: &str) -> SkillInfo {
        SkillInfo {
            name: name.into(),
            description: description.into(),
            path: String::new(),
            enabled: true,
            source: "user".into(),
        }
    }

    #[test]
    fn body_strips_well_formed_frontmatter() {
        let md = "---\nname: x\ndescription: y\n---\n\n## Body\n";
        assert_eq!(skill_body(md), "## Body\n");
    }

    #[test]
    fn body_handles_crlf_and_bom() {
        let md = "\u{feff}---\r\nname: x\r\n---\r\nBody\r\n";
        assert_eq!(skill_body(md), "Body\r\n");
    }

    #[test]
    fn body_without_frontmatter_is_unchanged() {
        assert_eq!(skill_body("just text"), "just text");
        assert_eq!(skill_body("--- not a fence"), "--- not a fence");
    }

    #[test]
    fn body_without_closing_fence_is_unchanged() {
        let md = "---\nname: x\nno closing fence";
        assert_eq!(skill_body(md), md);
    }

    #[test]
    fn filter_matches_name_and_description_case_insensitively() {
        let s = info("Git-Flow", "Release branching");
        assert!(matches_filter(&s, ""));
        assert!(matches_filter(&s, "git"));
        assert!(matches_filter(&s, "branch"));
        assert!(!matches_filter(&s, "docker"));
    }
}
