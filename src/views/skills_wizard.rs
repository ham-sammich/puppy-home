//! The guided create/edit wizard for the Skills Manager (modal window).
//!
//! Three steps: basics (name, description, scope), content (markdown body),
//! review (the exact SKILL.md that lands on disk). The manager owns the
//! `Wizard` state and acts on the returned [`WizardAction`].

use eframe::egui;

use crate::views::common::validate_name;

/// Where a skill is saved (mirrors the sidecar's `save_skill` scopes).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    pub fn wire(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::User => "User (all projects)",
            Scope::Project => "Project (this folder)",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Scope::User => "Saved under ~/.code_puppy/skills.",
            Scope::Project => "Saved under ./.code_puppy/skills in the serving workspace.",
        }
    }
}

/// Map a catalog row's source to the scope it would be saved under.
pub fn scope_for_source(source: &str) -> Scope {
    if source == "project" {
        Scope::Project
    } else {
        Scope::User
    }
}

/// Scaffold body for a new skill (the sidecar adds the frontmatter).
const TEMPLATE_BODY: &str = "## When to use this skill\n\n\
Describe the situations where this skill applies.\n\n\
## Instructions\n\n\
1. Step one.\n\
2. Step two.\n\n\
## Examples\n\n\
Show a short example of the skill in action.\n";

/// The wizard's state (3 steps: basics, content, review).
pub struct Wizard {
    step: usize,
    pub name: String,
    pub description: String,
    /// The markdown body (the sidecar adds the frontmatter).
    pub content: String,
    pub scope: Scope,
    /// `true` when opened from "Edit" (changes the title and button wording).
    editing: bool,
    error: Option<String>,
}

impl Wizard {
    pub fn create() -> Self {
        Wizard {
            step: 0,
            name: String::new(),
            description: String::new(),
            content: TEMPLATE_BODY.to_string(),
            scope: Scope::User,
            editing: false,
            error: None,
        }
    }

    pub fn edit(name: &str, description: &str, body: &str, scope: Scope) -> Self {
        Wizard {
            step: 0,
            name: name.to_string(),
            description: description.to_string(),
            content: body.to_string(),
            scope,
            editing: true,
            error: None,
        }
    }

    fn title(&self) -> &'static str {
        if self.editing {
            "Edit skill"
        } else {
            "Create skill"
        }
    }
}

/// What the manager should do after this frame's wizard render.
#[derive(PartialEq, Eq)]
pub enum WizardAction {
    KeepOpen,
    Cancel,
    /// The user confirmed the review step: send `save_skill` and close.
    Save,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Assemble the full SKILL.md for the review step; mirrors the sidecar's
/// `_compose_skill_md` so the user reviews what actually lands on disk.
fn compose_preview(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        name.trim(),
        description.trim(),
        body.trim_end()
    )
}

/// Validate the basics step; mirrors the sidecar's checks so the user hears
/// about problems before the op crosses the wire.
fn validate_basics(w: &Wizard) -> Result<(), String> {
    validate_name(&w.name)?;
    if w.description.trim().is_empty() {
        return Err("a description is required (it's how agents find the skill)".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the modal; Esc cancels. The caller owns the open/close lifecycle.
pub fn show(ctx: &egui::Context, wizard: &mut Wizard) -> WizardAction {
    let mut action = WizardAction::KeepOpen;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = WizardAction::Cancel;
    }

    egui::Window::new(wizard.title())
        .id(egui::Id::new("skills-wizard"))
        .collapsible(false)
        .resizable(false)
        // Anchor with hard margins (like the MCP wizard) so the window can
        // never grow off-screen.
        .anchor(egui::Align2::LEFT_TOP, [16.0, 48.0])
        .show(ctx, |ui| {
            let screen = ui.ctx().content_rect();
            let w = (screen.width() - 32.0).clamp(320.0, 640.0);
            let h = (screen.height() - 96.0).clamp(240.0, 560.0);
            ui.set_min_width(w);
            ui.set_max_size(egui::vec2(w, h));

            let steps = ["Basics", "Content", "Review"];
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
                0 => step_basics(ui, wizard),
                1 => step_content(ui, wizard),
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
                    action = WizardAction::Cancel;
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match wizard.step {
                        0 => {
                            if ui.button("Next").clicked() {
                                match validate_basics(wizard) {
                                    Ok(()) => {
                                        wizard.step = 1;
                                        wizard.error = None;
                                    }
                                    Err(e) => wizard.error = Some(e),
                                }
                            }
                        }
                        1 => {
                            if ui.button("Next").clicked() {
                                wizard.step = 2;
                                wizard.error = None;
                            }
                            if ui.button("Back").clicked() {
                                wizard.step = 0;
                                wizard.error = None;
                            }
                        }
                        _ => {
                            if ui.button("Save skill").clicked() {
                                action = WizardAction::Save;
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

    action
}

fn step_basics(ui: &mut egui::Ui, wizard: &mut Wizard) {
    egui::Grid::new("skills-wizard-basics")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("my-skill (letters, digits, - and _)"),
            );
            ui.end_row();

            ui.label("Description");
            ui.add(
                egui::TextEdit::singleline(&mut wizard.description)
                    .desired_width(f32::INFINITY)
                    .hint_text("one line: when should an agent reach for this?"),
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
            "Saving writes <skills dir>/<name>/SKILL.md - keep the same name \
             and scope to overwrite in place.",
        );
    }
}

fn step_content(ui: &mut egui::Ui, wizard: &mut Wizard) {
    ui.label("SKILL.md body (markdown; the frontmatter is added on save):");
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .max_height(380.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut wizard.content)
                    .desired_rows(16)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
}

fn step_review(ui: &mut egui::Ui, wizard: &Wizard) {
    ui.label("Review and confirm:");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(wizard.name.trim());
        ui.label(egui::RichText::new(wizard.scope.wire()).weak());
    });
    let preview = compose_preview(&wizard.name, &wizard.description, &wizard.content);
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview.as_str())
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
    ui.weak(if wizard.editing {
        "Saving overwrites the existing SKILL.md."
    } else {
        "The skill is discovered immediately; toggle it off any time."
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_mirrors_sidecar_compose() {
        let p = compose_preview(" git-flow ", " Release flow ", "## Steps\n\n");
        assert_eq!(
            p,
            "---\nname: git-flow\ndescription: Release flow\n---\n\n## Steps\n"
        );
    }

    #[test]
    fn basics_validation_requires_name_and_description() {
        let mut w = Wizard::create();
        assert!(validate_basics(&w).is_err()); // empty name
        w.name = "ok-skill".into();
        assert!(validate_basics(&w).is_err()); // empty description
        w.description = "does things".into();
        assert!(validate_basics(&w).is_ok());
        w.name = "../escape".into();
        assert!(validate_basics(&w).is_err()); // traversal shape rejected
    }

    #[test]
    fn scope_follows_source() {
        assert_eq!(scope_for_source("project").wire(), "project");
        assert_eq!(scope_for_source("user").wire(), "user");
        // plugin skills re-save as user copies
        assert_eq!(scope_for_source("plugin").wire(), "user");
        assert_eq!(scope_for_source("").wire(), "user");
    }

    #[test]
    fn create_wizard_seeds_the_template() {
        let w = Wizard::create();
        assert!(w.content.contains("## When to use this skill"));
        assert_eq!(w.title(), "Create skill");
        assert_eq!(
            Wizard::edit("a", "b", "c", Scope::Project).title(),
            "Edit skill"
        );
    }
}
