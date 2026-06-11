//! The guided visual builder for the Agent Manager (modal window).
//!
//! Four steps: basics (name, display name, description, model, scope), prompt
//! (system + optional user prompt), tools (built-in tool + MCP server
//! selection), review (the exact JSON that lands on disk). The manager owns
//! the [`Wizard`] state and acts on the returned [`WizardAction`].

use eframe::egui;

use crate::backend::{AgentConfigDetail, AgentConfigDraft};
use crate::views::common::validate_name;

/// Where an agent JSON file is saved (mirrors the sidecar's save scopes).
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
            Scope::User => "Saved under ~/.code_puppy/agents.",
            Scope::Project => "Saved under ./.code_puppy/agents in the serving workspace.",
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

const TEMPLATE_PROMPT: &str = "You are a focused coding assistant.\n\n\
Describe the agent's role, what it is good at, and any rules it must follow.";

/// The wizard's state (4 steps: basics, prompt, tools, review).
pub struct Wizard {
    step: usize,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model: String,
    pub system_prompt: String,
    pub user_prompt: String,
    /// Selected built-in tool names.
    pub tools: Vec<String>,
    /// Selected MCP server bindings.
    pub mcp_servers: Vec<String>,
    pub scope: Scope,
    /// Tool-list filter (basics-step ergonomics; not persisted).
    tool_filter: String,
    /// `true` when opened from "Edit" (changes title and button wording).
    editing: bool,
    error: Option<String>,
}

impl Wizard {
    pub fn create() -> Self {
        Wizard {
            step: 0,
            name: String::new(),
            display_name: String::new(),
            description: String::new(),
            model: String::new(),
            system_prompt: TEMPLATE_PROMPT.to_string(),
            user_prompt: String::new(),
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            scope: Scope::User,
            tool_filter: String::new(),
            editing: false,
            error: None,
        }
    }

    /// Seed the builder from a fetched config (the "Edit" path).
    pub fn edit(detail: &AgentConfigDetail) -> Self {
        Wizard {
            step: 0,
            name: detail.name.clone(),
            display_name: detail.display_name.clone(),
            description: detail.description.clone(),
            model: detail.model.clone(),
            system_prompt: detail.system_prompt.clone(),
            user_prompt: detail.user_prompt.clone().unwrap_or_default(),
            tools: detail.tools.clone(),
            mcp_servers: detail.mcp_servers.clone(),
            scope: scope_for_source(&detail.source),
            tool_filter: String::new(),
            editing: true,
            error: None,
        }
    }

    fn title(&self) -> &'static str {
        if self.editing {
            "Edit agent"
        } else {
            "Create agent"
        }
    }

    /// Build the draft the sidecar's `save_agent_config` expects.
    pub fn draft(&self) -> AgentConfigDraft {
        AgentConfigDraft {
            name: self.name.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            description: self.description.trim().to_string(),
            system_prompt: self.system_prompt.clone(),
            user_prompt: self.user_prompt.clone(),
            model: self.model.trim().to_string(),
            tools: self.tools.clone(),
            mcp_servers: self.mcp_servers.clone(),
            scope: self.scope.wire().to_string(),
        }
    }
}

/// What the manager should do after this frame's wizard render.
#[derive(PartialEq, Eq)]
pub enum WizardAction {
    KeepOpen,
    Cancel,
    /// The user confirmed the review step: send `save_agent_config` and close.
    Save,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Escape one string as a JSON scalar (matches Python `json.dumps` for the
/// common cases: quotes, backslashes, control chars; non-ASCII kept literal).
fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// Format a string array the way `json.dumps(indent=2)` nests it under a
/// 2-space key (empty -> `[]`, else one element per line at 4 spaces).
fn json_array(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let inner: Vec<String> = items
        .iter()
        .map(|i| format!("    {}", json_str(i)))
        .collect();
    format!("[\n{}\n  ]", inner.join(",\n"))
}

/// Assemble the on-disk agent JSON for the review step; mirrors the sidecar's
/// `json.dumps(config, indent=2)` field order and optional-field omission so
/// the user reviews exactly what lands on disk.
fn compose_preview(w: &Wizard) -> String {
    let mut entries: Vec<(&str, String)> = vec![
        ("name", json_str(w.name.trim())),
        ("description", json_str(w.description.trim())),
        ("system_prompt", json_str(&w.system_prompt)),
        ("tools", json_array(&w.tools)),
    ];
    if !w.display_name.trim().is_empty() {
        entries.push(("display_name", json_str(w.display_name.trim())));
    }
    if !w.model.trim().is_empty() {
        entries.push(("model", json_str(w.model.trim())));
    }
    if !w.user_prompt.trim().is_empty() {
        entries.push(("user_prompt", json_str(&w.user_prompt)));
    }
    if !w.mcp_servers.is_empty() {
        entries.push(("mcp_servers", json_array(&w.mcp_servers)));
    }
    let body: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("  {}: {}", json_str(k), v))
        .collect();
    format!("{{\n{}\n}}", body.join(",\n"))
}

/// Validate the basics step; mirrors the sidecar's checks so the user hears
/// about problems before the op crosses the wire.
fn validate_basics(w: &Wizard) -> Result<(), String> {
    validate_name(&w.name)?;
    if w.description.trim().is_empty() {
        return Err("a description is required (it's how the agent is summarised)".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the modal; Esc cancels. The caller owns the open/close lifecycle and
/// supplies the available tool/MCP catalogs for the selection step.
pub fn show(
    ctx: &egui::Context,
    wizard: &mut Wizard,
    tool_catalog: &[String],
    mcp_catalog: &[String],
) -> WizardAction {
    let mut action = WizardAction::KeepOpen;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = WizardAction::Cancel;
    }

    egui::Window::new(wizard.title())
        .id(egui::Id::new("agent-wizard"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::LEFT_TOP, [16.0, 48.0])
        .show(ctx, |ui| {
            let screen = ui.ctx().content_rect();
            let w = (screen.width() - 32.0).clamp(320.0, 680.0);
            let h = (screen.height() - 96.0).clamp(240.0, 600.0);
            ui.set_min_width(w);
            ui.set_max_size(egui::vec2(w, h));

            let steps = ["Basics", "Prompt", "Tools", "Review"];
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
                1 => step_prompt(ui, wizard),
                2 => step_tools(ui, wizard, tool_catalog, mcp_catalog),
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    nav_buttons(ui, wizard, &mut action);
                });
            });
        });

    action
}

/// The Next/Back/Save navigation footer for the current step.
fn nav_buttons(ui: &mut egui::Ui, wizard: &mut Wizard, action: &mut WizardAction) {
    match wizard.step {
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
        1 | 2 => {
            if ui.button("Next").clicked() {
                wizard.step += 1;
                wizard.error = None;
            }
            if ui.button("Back").clicked() {
                wizard.step -= 1;
                wizard.error = None;
            }
        }
        _ => {
            if ui.button("Save agent").clicked() {
                *action = WizardAction::Save;
            }
            if ui.button("Back").clicked() {
                wizard.step = 2;
                wizard.error = None;
            }
        }
    }
}

fn step_basics(ui: &mut egui::Ui, wizard: &mut Wizard) {
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

fn step_prompt(ui: &mut egui::Ui, wizard: &mut Wizard) {
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

fn step_tools(
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
fn toggle(set: &mut Vec<String>, item: &str, on: bool) {
    if on {
        if !set.iter().any(|x| x == item) {
            set.push(item.to_string());
        }
    } else {
        set.retain(|x| x != item);
    }
}

fn step_review(ui: &mut egui::Ui, wizard: &Wizard) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wizard_with(tools: &[&str], mcp: &[&str]) -> Wizard {
        let mut w = Wizard::create();
        w.name = " my-bot ".into();
        w.description = " Does things ".into();
        w.system_prompt = "You are helpful.".into();
        w.tools = tools.iter().map(|s| s.to_string()).collect();
        w.mcp_servers = mcp.iter().map(|s| s.to_string()).collect();
        w
    }

    #[test]
    fn preview_required_fields_only() {
        let w = wizard_with(&["list_files"], &[]);
        let expected = "{\n  \"name\": \"my-bot\",\n  \"description\": \"Does things\",\n  \
            \"system_prompt\": \"You are helpful.\",\n  \"tools\": [\n    \"list_files\"\n  ]\n}";
        assert_eq!(compose_preview(&w), expected);
    }

    #[test]
    fn preview_includes_optional_fields_in_order() {
        let mut w = wizard_with(&["a", "b"], &["serena"]);
        w.display_name = "My Bot".into();
        w.model = "gpt".into();
        w.user_prompt = "hi".into();
        let p = compose_preview(&w);
        // Field order mirrors the sidecar's json.dumps insertion order.
        let pos = |k: &str| p.find(&format!("\"{k}\"")).unwrap();
        assert!(pos("name") < pos("description"));
        assert!(pos("tools") < pos("display_name"));
        assert!(pos("display_name") < pos("model"));
        assert!(pos("model") < pos("user_prompt"));
        assert!(pos("user_prompt") < pos("mcp_servers"));
        assert!(p.contains("\"mcp_servers\": [\n    \"serena\"\n  ]"));
    }

    #[test]
    fn empty_tools_render_inline() {
        assert_eq!(json_array(&[]), "[]");
    }

    #[test]
    fn basics_validation_requires_name_and_description() {
        let mut w = Wizard::create();
        assert!(validate_basics(&w).is_err()); // empty name
        w.name = "ok-bot".into();
        assert!(validate_basics(&w).is_err()); // empty description
        w.description = "does things".into();
        assert!(validate_basics(&w).is_ok());
        w.name = "../escape".into();
        assert!(validate_basics(&w).is_err()); // traversal shape rejected
    }

    #[test]
    fn toggle_keeps_unique_set() {
        let mut set = vec!["a".to_string()];
        toggle(&mut set, "a", true); // already present, no dup
        toggle(&mut set, "b", true);
        assert_eq!(set, vec!["a".to_string(), "b".to_string()]);
        toggle(&mut set, "a", false);
        assert_eq!(set, vec!["b".to_string()]);
    }

    #[test]
    fn draft_trims_and_carries_scope() {
        let w = wizard_with(&["list_files"], &["serena"]);
        let d = w.draft();
        assert_eq!(d.name, "my-bot");
        assert_eq!(d.description, "Does things");
        assert_eq!(d.scope, "user");
        assert_eq!(d.tools, vec!["list_files".to_string()]);
        assert_eq!(d.mcp_servers, vec!["serena".to_string()]);
    }
}
