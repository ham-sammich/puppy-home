//! App-wide managers: MCP servers, Skills, Agents — lists + wizards over
//! the sidecar config ops, dressed in the GPUI tokens. Ports the egui
//! views (`views/{mcp,skills,agent}_manager` + wizards) reusing their
//! frontend-agnostic state machines (`mcp_wizard::Wizard`,
//! `skills_wizard::Wizard`, `agent_wizard::Wizard` — paste parse/validate,
//! review compose, scope mapping all included).
//!
//! egui-parity mechanics live here too: the visible-tab poll cadence
//! (request gap then slow refresh), generation tracking that clears
//! optimistic toggle overrides and re-fetches the open detail, and the
//! serving-workspace invariant (global config flows through the first
//! ready workspace's sidecar).
//!
//! Pattern notes (GPUI_NOTES.md): one overlay at a time; a small POOL of
//! input entities reused across forms (seeded when a form/step opens, read
//! back on advance/submit — no per-keystroke field sync); the raw-paste
//! buffer is one shared code-mode input with syntect highlighting (JSON
//! for MCP/agents, markdown for SKILL.md).
//!
//! Per-kind dispatch + render live in `managers_mcp` / `managers_skills` /
//! `managers_agents`; the overlay frame + shared widgets in `managers_ui`.

use std::time::{Duration, Instant};

use gpui::prelude::*;

use crate::gpui_ui::RootView;
use crate::gpui_ui::input::ChatInput;
use crate::workspace::{Workspace, WorkspaceId};

/// Minimum gap between polls (avoids spamming while the first answer is due).
const REQUEST_GAP: Duration = Duration::from_secs(2);
/// Re-poll cadence while the MCP manager is open (server state settles async).
const MCP_REFRESH: Duration = Duration::from_secs(5);
/// Re-poll cadence for skills/agents (they change rarely).
const SLOW_REFRESH: Duration = Duration::from_secs(10);

/// Which manager overlay is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MgrKind {
    Mcp,
    Skills,
    Agents,
    Models,
    Config,
}

impl MgrKind {
    pub fn title(self) -> &'static str {
        match self {
            MgrKind::Mcp => "MCP Servers",
            MgrKind::Skills => "Skills",
            MgrKind::Agents => "Agents",
            MgrKind::Models => "Models",
            MgrKind::Config => "Puppy Config",
        }
    }

    /// The egui tab subtitle base ("… - via {workspace}" is appended in UI).
    pub fn subtitle(self) -> &'static str {
        match self {
            MgrKind::Mcp => "global Code Puppy config",
            MgrKind::Skills => "user + project skills",
            MgrKind::Agents => "JSON + built-in agents",
            MgrKind::Models => "sidecar catalog + extra_models.json",
            MgrKind::Config => "puppy.cfg \u{2014} global settings",
        }
    }
}

/// Manager interactions, nested under DashAction::Mgr.
#[derive(Clone, Debug)]
pub enum MgrAction {
    Open(MgrKind),
    Close,
    Refresh,
    // mcp
    McpToggle(String, bool),
    McpWizardOpen,
    McpWizardCancel,
    McpTransport(u8),
    McpMode(bool), // true = paste
    McpStep(i32),
    McpFormat,
    McpSubmit,
    // skills
    SkillToggle(String, bool),
    SkillSelect(String),
    SkillWizardOpen(bool), // edit current selection?
    SkillWizardCancel,
    SkillMode(bool),
    SkillStep(i32),
    SkillScope(bool), // true = project
    SkillFormat,
    SkillSubmit,
    // agents
    AgentSelect(String),
    AgentClone(String),
    AgentDelete(String),
    AgentDeleteConfirm,
    AgentDeleteCancel,
    AgentWizardOpen(bool), // edit current selection?
    AgentWizardCancel,
    AgentMode(bool),
    AgentStep(i32),
    AgentScope(bool), // true = project
    AgentToggleTool(String),
    AgentToggleMcp(String),
    AgentToolsAll,
    AgentToolsNone,
    AgentFormat,
    AgentSubmit,
    /// Spawn a fresh home-dir session driven by code_puppy's built-in
    /// `agent-creator` agent (QW7) — conversational agent building.
    AgentCreatorOpen,
    // models (QW4)
    ModelSetActive(String),
    ModelsEditorOpen,
    ModelsEditorCancel,
    ModelsEditorSave,
    ModelRemove(String),
    // config (QW5)
    CfgEdit(String),
    CfgEditCancel,
    CfgEditSave,
}

// Field-pool indices (one manager open at a time -> safe reuse).
/// The list filter (kept apart from wizard fields so it survives a wizard).
pub const F_FILTER: usize = 0;
pub const F_NAME: usize = 1;
pub const F_B: usize = 2; // command / description / display_name
pub const F_C: usize = 3; // args / content / description
pub const F_D: usize = 4; // env lines / model
pub const F_E: usize = 5; // url / system prompt
pub const F_F: usize = 6; // header lines / user prompt
pub const F_TOOLF: usize = 7; // agent-wizard tool filter
const POOL: usize = 8;

/// Syntect grammar hook for the shared paste buffer.
pub(crate) fn paste_file(kind: Option<MgrKind>) -> &'static str {
    match kind {
        Some(MgrKind::Skills) => "SKILL.md",
        _ => "x.json",
    }
}

impl RootView {
    /// First workspace with a ready sidecar (managers talk through it) —
    /// the egui `serving_workspace` invariant.
    pub(crate) fn first_ready_ws(&self) -> Option<WorkspaceId> {
        self.supervisor.iter().find(|w| w.is_ready()).map(|w| w.id)
    }

    pub(crate) fn serving_ws(&self) -> Option<&Workspace> {
        self.supervisor.iter().find(|w| w.is_ready())
    }

    pub(crate) fn ensure_mgr_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        while self.mgr_inputs.len() < POOL {
            let entity = cx.new(|cx| ChatInput::new("", cx));
            let sub = cx.subscribe(&entity, |_, _, _: &crate::gpui_ui::InputEvent, cx| {
                cx.notify()
            });
            self.mgr_inputs.push(entity);
            self.chat_subs.push(sub);
        }
        if self.mgr_paste_input.is_none() {
            let entity = cx.new(ChatInput::new_code);
            let sub = cx.subscribe(
                &entity,
                |this: &mut Self, input, ev: &crate::gpui_ui::InputEvent, cx| {
                    if matches!(ev, crate::gpui_ui::InputEvent::Edited) {
                        // Live highlighting for the paste buffer (JSON or md).
                        let text = input.read(cx).text().to_string();
                        let runs = crate::gpui_ui::editor::highlight(
                            &text,
                            std::path::Path::new(paste_file(this.manager_open)),
                            this.tokens.dark,
                        );
                        input.update(cx, |i, cx| i.set_syntax(runs, cx));
                    }
                    cx.notify();
                },
            );
            self.mgr_paste_input = Some(entity);
            self.chat_subs.push(sub);
        }
    }

    pub(crate) fn mgr_input_text(&self, ix: usize, cx: &gpui::Context<Self>) -> String {
        self.mgr_inputs
            .get(ix)
            .map(|i| i.read(cx).text().to_string())
            .unwrap_or_default()
    }

    pub(crate) fn seed(&self, ix: usize, text: String, cx: &mut gpui::Context<Self>) {
        if let Some(input) = self.mgr_inputs.get(ix) {
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }

    pub(crate) fn paste_text(&self, cx: &gpui::Context<Self>) -> String {
        self.mgr_paste_input
            .as_ref()
            .map(|i| i.read(cx).text().to_string())
            .unwrap_or_default()
    }

    /// Seed the paste buffer (text + a fresh highlight pass).
    pub(crate) fn seed_paste(&self, text: String, cx: &mut gpui::Context<Self>) {
        if let Some(input) = &self.mgr_paste_input {
            let runs = crate::gpui_ui::editor::highlight(
                &text,
                std::path::Path::new(paste_file(self.manager_open)),
                self.tokens.dark,
            );
            input.update(cx, |i, cx| {
                i.set_text(text, cx);
                i.set_syntax(runs, cx);
            });
        }
    }

    pub(crate) fn dispatch_mgr(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        use MgrAction::*;
        match action {
            Open(kind) => {
                self.ensure_mgr_inputs(cx);
                self.manager_open = Some(kind);
                self.models_editor = false;
                self.cfg_edit_key = None;
                if kind == MgrKind::Config {
                    self.cfg_reload();
                }
                self.mgr_selected = None;
                self.agent_delete_confirm = None;
                self.mcp_wizard = None;
                self.skills_wizard = None;
                self.agent_wizard = None;
                self.mgr_seen = None;
                self.mgr_last_request = None;
                self.mgr_pending.clear();
                self.seed(F_FILTER, String::new(), cx);
                self.mgr_upkeep(); // request the list immediately
            }
            Close => {
                self.manager_open = None;
                self.mcp_wizard = None;
                self.skills_wizard = None;
                self.agent_wizard = None;
            }
            Refresh => {
                if self.manager_open == Some(MgrKind::Config) {
                    self.cfg_reload();
                }
                self.mgr_last_request = None;
                self.mgr_upkeep();
            }
            a @ (McpToggle(..) | McpWizardOpen | McpWizardCancel | McpTransport(_) | McpMode(_)
            | McpStep(_) | McpFormat | McpSubmit) => self.dispatch_mcp(a, cx),
            a @ (SkillToggle(..) | SkillSelect(_) | SkillWizardOpen(_) | SkillWizardCancel
            | SkillMode(_) | SkillStep(_) | SkillScope(_) | SkillFormat | SkillSubmit) => {
                self.dispatch_skills(a, cx)
            }
            a @ (ModelSetActive(_) | ModelsEditorOpen | ModelsEditorCancel | ModelsEditorSave
            | ModelRemove(_)) => self.dispatch_models(a, cx),
            a @ (CfgEdit(_) | CfgEditCancel | CfgEditSave) => self.dispatch_config(a, cx),
            a => self.dispatch_agents(a, cx),
        }
        cx.notify();
    }

    /// Visible-overlay upkeep, ridden by the drain loop: poll on the egui
    /// cadences, and on a catalog generation bump clear optimistic toggles +
    /// re-fetch the open detail (its content may have just changed).
    pub(crate) fn mgr_upkeep(&mut self) {
        let Some(kind) = self.manager_open else {
            return;
        };
        // Models reads the always-maintained catalog; Config reads a local
        // file — neither polls the sidecar.
        if matches!(kind, MgrKind::Models | MgrKind::Config) {
            return;
        }
        let Some((ws_id, generation, have)) = self.serving_ws().map(|ws| match kind {
            MgrKind::Mcp => (ws.id, ws.mcp_generation, ws.mcp_servers.is_some()),
            MgrKind::Skills => (ws.id, ws.skills_generation, ws.skills.is_some()),
            MgrKind::Agents => (
                ws.id,
                ws.agent_configs_generation,
                ws.agent_configs.is_some(),
            ),
            MgrKind::Models | MgrKind::Config => unreachable!("early-returned above"),
        }) else {
            return;
        };
        if self.mgr_seen != Some((ws_id, generation)) {
            if self.mgr_seen.map(|(id, _)| id) != Some(ws_id) {
                self.mgr_last_request = None; // new source: request immediately
            }
            self.mgr_seen = Some((ws_id, generation));
            self.mgr_pending.clear();
            if let Some(sel) = self.mgr_selected.clone() {
                self.with_ready_backend(|b| match kind {
                    MgrKind::Skills => b.get_skill(&sel),
                    MgrKind::Agents => b.get_agent_config(&sel),
                    MgrKind::Mcp | MgrKind::Models | MgrKind::Config => {}
                });
            }
        }
        let need = if have {
            match kind {
                MgrKind::Mcp => MCP_REFRESH,
                _ => SLOW_REFRESH,
            }
        } else {
            REQUEST_GAP
        };
        let stale = self.mgr_last_request.is_none_or(|at| at.elapsed() >= need);
        if stale {
            self.with_ready_backend(|b| match kind {
                MgrKind::Mcp => b.list_mcp_servers(),
                MgrKind::Skills => b.list_skills(),
                MgrKind::Agents => b.list_agent_configs(),
                MgrKind::Models | MgrKind::Config => {}
            });
            self.mgr_last_request = Some(Instant::now());
        }
    }

    pub(crate) fn with_ready_backend(&self, f: impl FnOnce(&crate::backend::CodePuppy)) {
        if let Some(ws) = self.serving_ws()
            && let Some(backend) = &ws.backend
        {
            f(backend);
        }
    }
}

/// `KEY=VALUE` per line <-> pair vec (a deliberate simplification of the
/// egui wizards' add/remove pair rows — documented deviation).
pub(crate) fn pairs_to_lines(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn lines_to_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_lines_roundtrip() {
        let pairs = vec![
            ("API_KEY".to_string(), "abc=123".to_string()),
            ("MODE".to_string(), "fast".to_string()),
        ];
        let lines = pairs_to_lines(&pairs);
        assert_eq!(lines, "API_KEY=abc=123\nMODE=fast");
        assert_eq!(lines_to_pairs(&lines), pairs); // split_once keeps '=' in values
        assert!(lines_to_pairs("  \n no-equals \n").is_empty());
    }

    #[test]
    fn paste_grammar_follows_kind() {
        assert_eq!(paste_file(Some(MgrKind::Skills)), "SKILL.md");
        assert_eq!(paste_file(Some(MgrKind::Mcp)), "x.json");
        assert_eq!(paste_file(Some(MgrKind::Agents)), "x.json");
        assert_eq!(paste_file(None), "x.json");
    }
}
