//! The guided create/edit wizard for the Skills Manager (modal window).
//!
//! Three steps: basics (name, description, scope), content (markdown body),
//! review (the exact SKILL.md that lands on disk). The manager owns the
//! `Wizard` state and acts on the returned [`WizardAction`].

use crate::views::common::{EditMode, validate_name};

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

    pub(crate) fn label(self) -> &'static str {
        match self {
            Scope::User => "User (all projects)",
            Scope::Project => "Project (this folder)",
        }
    }

    pub(crate) fn blurb(self) -> &'static str {
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
    // Non-pub fields are pub(crate) so the GPUI manager drives the same
    // state machine (sync note: mirror on the egui branch at batch time).
    pub(crate) step: usize,
    pub name: String,
    pub description: String,
    /// The markdown body (the sidecar adds the frontmatter).
    pub content: String,
    pub scope: Scope,
    /// `true` when opened from "Edit" (changes the title and button wording).
    pub(crate) editing: bool,
    pub(crate) error: Option<String>,
    /// Form (guided steps) vs. Paste (drop in a whole SKILL.md and validate).
    pub(crate) mode: EditMode,
    /// The raw-paste buffer (a full SKILL.md), seeded from the form on entry.
    pub(crate) paste: String,
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
            mode: EditMode::Form,
            paste: String::new(),
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
            mode: EditMode::Form,
            paste: String::new(),
        }
    }

    /// Seed the paste buffer from the current form fields (canonical SKILL.md).
    pub(crate) fn sync_paste_from_form(&mut self) {
        self.paste = compose_preview(&self.name, &self.description, &self.content);
    }

    /// Parse the paste buffer back into the form fields (the syntax check).
    pub(crate) fn apply_paste(&mut self) -> Result<(), String> {
        let (name, description, body) = parse_skill_md(&self.paste)?;
        self.name = name;
        self.description = description;
        self.content = body;
        Ok(())
    }

    pub(crate) fn title(&self) -> &'static str {
        if self.editing {
            "Edit skill"
        } else {
            "Create skill"
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Assemble the full SKILL.md for the review step; mirrors the sidecar's
/// `_compose_skill_md` so the user reviews what actually lands on disk.
pub(crate) fn compose_preview(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        name.trim(),
        description.trim(),
        body.trim_end()
    )
}

/// Validate the basics step; mirrors the sidecar's checks so the user hears
/// about problems before the op crosses the wire.
pub(crate) fn validate_basics(w: &Wizard) -> Result<(), String> {
    validate_name(&w.name)?;
    if w.description.trim().is_empty() {
        return Err("a description is required (it's how agents find the skill)".into());
    }
    Ok(())
}

/// Parse a full SKILL.md (YAML-ish frontmatter + markdown body) into
/// `(name, description, body)`. Mirrors the sidecar's loader and validates the
/// required keys, so a bad paste fails here rather than on disk.
fn parse_skill_md(text: &str) -> Result<(String, String, String), String> {
    let rest = text
        .trim_start()
        .strip_prefix("---")
        .ok_or("missing '---' frontmatter block at the top")?;
    let close = rest
        .find("\n---")
        .ok_or("frontmatter is never closed with a '---' line")?;
    let (frontmatter, after) = (&rest[..close], &rest[close + 4..]);
    let body = after.trim_start_matches(['\r', '\n']);

    let mut name = String::new();
    let mut description = String::new();
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().to_string();
        }
    }
    validate_name(&name)?;
    if description.trim().is_empty() {
        return Err("frontmatter needs a non-empty 'description:'".into());
    }
    Ok((name, description, body.trim_end().to_string()))
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_compose() {
        let md = compose_preview("git-flow", "Release flow", "## Steps\n\n1. go");
        let (name, desc, body) = parse_skill_md(&md).unwrap();
        assert_eq!(name, "git-flow");
        assert_eq!(desc, "Release flow");
        assert_eq!(body, "## Steps\n\n1. go");
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse_skill_md("no frontmatter here").is_err());
        assert!(parse_skill_md("---\nname: x\n").is_err()); // unclosed
        assert!(parse_skill_md("---\nname: x\n---\nbody").is_err()); // no description
        assert!(parse_skill_md("---\ndescription: y\n---\nbody").is_err()); // no name
        assert!(parse_skill_md("---\nname: bad/name\ndescription: y\n---\nb").is_err());
    }

    #[test]
    fn parse_tolerates_leading_blank_lines_and_crlf() {
        let md = "\n\n---\r\nname: ok\r\ndescription: d\r\n---\r\n\r\nbody text\n";
        let (name, desc, body) = parse_skill_md(md).unwrap();
        assert_eq!(name, "ok");
        assert_eq!(desc, "d");
        assert_eq!(body, "body text");
    }

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
