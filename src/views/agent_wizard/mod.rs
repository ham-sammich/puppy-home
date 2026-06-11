//! The guided visual builder for the Agent Manager (modal window).
//!
//! Two ways in: a guided **Form** (basics, prompt, tools, review) or a raw
//! **Paste** mode where you drop in a whole agent JSON and validate/format it.
//! Both funnel through the same [`AgentConfigDraft`] -> `save_agent_config`.
//! The per-step renderers live in the `steps` child module.

mod steps;

use eframe::egui;
use serde_json::Value;

use crate::backend::{AgentConfigDetail, AgentConfigDraft};
use crate::views::common::{EditMode, mode_toggle, validate_name};

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
    /// Form (guided steps) vs. Paste (drop in a whole agent JSON and validate).
    mode: EditMode,
    /// The raw-paste buffer (a full agent JSON), seeded from the form on entry.
    paste: String,
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
            mode: EditMode::Form,
            paste: String::new(),
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
            mode: EditMode::Form,
            paste: String::new(),
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

    /// Seed the paste buffer from the current form fields (canonical JSON).
    fn sync_paste_from_form(&mut self) {
        self.paste = compose_preview(self);
    }

    /// Parse the paste buffer back into the form fields (the syntax check).
    fn apply_paste(&mut self) -> Result<(), String> {
        let p = parse_agent_json(&self.paste)?;
        self.name = p.name;
        self.display_name = p.display_name;
        self.description = p.description;
        self.model = p.model;
        self.system_prompt = p.system_prompt;
        self.user_prompt = p.user_prompt;
        self.tools = p.tools;
        self.mcp_servers = p.mcp_servers;
        Ok(())
    }
}

/// What the manager should do after this frame's wizard render.
#[derive(PartialEq, Eq)]
pub enum WizardAction {
    KeepOpen,
    Cancel,
    /// The user confirmed: send `save_agent_config` and close.
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

/// The fields a pasted agent JSON parses into (then copied onto the wizard).
struct ParsedAgent {
    name: String,
    display_name: String,
    description: String,
    model: String,
    system_prompt: String,
    user_prompt: String,
    tools: Vec<String>,
    mcp_servers: Vec<String>,
}

/// A JSON array of strings (non-string entries ignored); `None`/missing -> [].
fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Agent `mcp_servers` entries may be bare strings or `{ "name": ... }`
/// objects (code-puppy accepts both); collect just the server names.
fn mcp_names(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    i.as_str()
                        .map(str::to_string)
                        .or_else(|| i.get("name").and_then(Value::as_str).map(str::to_string))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a pasted agent JSON into [`ParsedAgent`], mirroring the sidecar's
/// required-field checks so a bad paste fails here, not on disk.
fn parse_agent_json(text: &str) -> Result<ParsedAgent, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = v.as_object().ok_or("the top level must be a JSON object")?;
    let s = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let name = s("name");
    validate_name(&name)?;
    let description = s("description");
    if description.trim().is_empty() {
        return Err("a \"description\" field is required".into());
    }
    Ok(ParsedAgent {
        name,
        display_name: s("display_name"),
        description,
        model: s("model"),
        system_prompt: s("system_prompt"),
        user_prompt: s("user_prompt"),
        tools: str_array(obj.get("tools")),
        mcp_servers: mcp_names(obj.get("mcp_servers")),
    })
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

            // Form vs. Paste mode (seed the paste buffer on the way in).
            let next_mode = mode_toggle(ui, wizard.mode);
            if next_mode != wizard.mode {
                if next_mode == EditMode::Paste {
                    wizard.sync_paste_from_form();
                }
                wizard.mode = next_mode;
                wizard.error = None;
            }
            ui.separator();

            match wizard.mode {
                EditMode::Form => {
                    let labels = ["Basics", "Prompt", "Tools", "Review"];
                    ui.horizontal(|ui| {
                        for (i, label) in labels.iter().enumerate() {
                            let text = format!("{}. {label}", i + 1);
                            if i == wizard.step {
                                ui.strong(text);
                            } else {
                                ui.weak(text);
                            }
                            if i + 1 < labels.len() {
                                ui.weak(">");
                            }
                        }
                    });
                    ui.separator();
                    match wizard.step {
                        0 => steps::step_basics(ui, wizard),
                        1 => steps::step_prompt(ui, wizard),
                        2 => steps::step_tools(ui, wizard, tool_catalog, mcp_catalog),
                        _ => steps::step_review(ui, wizard),
                    }
                }
                EditMode::Paste => steps::step_paste(ui, wizard),
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
                    |ui| match wizard.mode {
                        EditMode::Paste => paste_footer(ui, wizard, &mut action),
                        EditMode::Form => nav_buttons(ui, wizard, &mut action),
                    },
                );
            });
        });

    action
}

/// Paste-mode footer: Save validates + writes; Format syntax-checks + tidies.
fn paste_footer(ui: &mut egui::Ui, wizard: &mut Wizard, action: &mut WizardAction) {
    if ui.button("Save agent").clicked() {
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

/// The Next/Back/Save navigation footer for the current form step.
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
    fn draft_trims_and_carries_scope() {
        let w = wizard_with(&["list_files"], &["serena"]);
        let d = w.draft();
        assert_eq!(d.name, "my-bot");
        assert_eq!(d.description, "Does things");
        assert_eq!(d.scope, "user");
        assert_eq!(d.tools, vec!["list_files".to_string()]);
        assert_eq!(d.mcp_servers, vec!["serena".to_string()]);
    }

    #[test]
    fn paste_round_trips_compose() {
        let mut w = wizard_with(&["list_files", "edit_file"], &["serena"]);
        w.display_name = "My Bot".into();
        w.model = "gpt".into();
        w.user_prompt = "hi".into();
        let json = compose_preview(&w);

        let mut blank = Wizard::create();
        blank.paste = json;
        blank.apply_paste().unwrap();
        assert_eq!(blank.name, "my-bot");
        assert_eq!(blank.display_name, "My Bot");
        assert_eq!(blank.model, "gpt");
        assert_eq!(
            blank.tools,
            vec!["list_files".to_string(), "edit_file".to_string()]
        );
        assert_eq!(blank.mcp_servers, vec!["serena".to_string()]);
    }

    #[test]
    fn paste_rejects_bad_input() {
        let mut w = Wizard::create();
        w.paste = "{ not json".into();
        assert!(w.apply_paste().is_err());
        w.paste = "[1, 2, 3]".into();
        assert!(w.apply_paste().is_err()); // not an object
        w.paste = "{\"name\": \"ok\"}".into();
        assert!(w.apply_paste().is_err()); // missing description
        w.paste = "{\"name\": \"bad/name\", \"description\": \"d\"}".into();
        assert!(w.apply_paste().is_err()); // bad name
    }

    #[test]
    fn paste_accepts_dict_form_mcp_servers() {
        let mut w = Wizard::create();
        w.paste = "{\"name\": \"ok\", \"description\": \"d\", \
                    \"mcp_servers\": [\"a\", {\"name\": \"b\", \"auto_start\": true}]}"
            .into();
        w.apply_paste().unwrap();
        assert_eq!(w.mcp_servers, vec!["a".to_string(), "b".to_string()]);
    }
}
