//! The dockable shell: tab descriptors, the `TabViewer`, and deferred actions.

use std::path::PathBuf;

use eframe::egui;
use egui_dock::TabViewer;
use egui_dock::widgets::tab_viewer::OnCloseResponse;

use crate::browser::{BrowserId, BrowserManager};
use crate::supervisor::Supervisor;
use crate::views;
use crate::workspace::{InstanceStatus, WorkspaceId};

/// A tab is a lightweight descriptor — heavy state lives in the supervisor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tab {
    Dashboard,
    Chat(WorkspaceId),
    /// The optional browser plugin's view.
    Browser(BrowserId),
    /// Code Puppy's MCP servers: list, toggle, add (one instance).
    McpManager,
    /// Code Puppy's skills: list, toggle, create/edit (one instance).
    SkillsManager,
    /// Code Puppy's agents: list, edit, clone, visual builder (one instance).
    AgentManager,
    /// Puppy Pack: presence + chat with teammates via a relay (one instance).
    Pack,
}

/// Structural changes requested during rendering, applied after the dock draws.
pub enum ShellAction {
    #[allow(dead_code)] // reserved action (matched in app.rs); not yet emitted
    OpenFolder(PathBuf),
    Close(WorkspaceId),
    FocusChat(WorkspaceId),
    /// Focus a workspace's chat tab and switch its editor to the Changes view.
    ShowChanges(WorkspaceId),
    /// Pause the running turn at the next safe boundary (card action).
    Pause(WorkspaceId),
    /// Resume a turn held at the pause gate (card action).
    Resume(WorkspaceId),
    /// Cancel the running turn (card action).
    Stop(WorkspaceId),
    /// Relaunch a dead sidecar and restore its session (card "Restart").
    Restart(WorkspaceId),
    /// Steer the running turn; `queue` delivers after the current turn.
    Steer {
        id: WorkspaceId,
        text: String,
        queue: bool,
    },
    /// Send a fresh prompt from a card (idle/done states).
    SendPrompt {
        id: WorkspaceId,
        text: String,
    },
    /// Live model switch via the card's model pill.
    SetModel {
        id: WorkspaceId,
        model: String,
    },
}

/// Transient `TabViewer` holding mutable access to app state for one frame.
pub struct Shell<'a> {
    pub sup: &'a mut Supervisor,
    pub browser: &'a mut BrowserManager,
    pub mcp: &'a mut views::mcp_manager::McpManagerView,
    pub skills: &'a mut views::skills_manager::SkillsManagerView,
    pub agents: &'a mut views::agent_manager::AgentManagerView,
    pub pack: &'a mut views::pack_panel::PackView,
    /// Dashboard view state (Grid/List/Focus, inline inputs, toasts).
    pub dashboard: &'a mut views::dashboard::DashboardView,
    /// Resolved brand/status colors for the active theme.
    pub accents: &'a crate::theme::Accents,
    /// The chat composer skin (user preference; the gear popover edits it).
    pub composer_style: &'a mut crate::session::ComposerStyle,
    pub actions: &'a mut Vec<ShellAction>,
}

impl TabViewer for Shell<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Dashboard => "📊 Dashboard".into(),
            Tab::Chat(id) => match self.sup.get(*id) {
                Some(w) => {
                    let mark = if w.status == InstanceStatus::WaitingForInput {
                        "● "
                    } else {
                        ""
                    };
                    format!("{mark}{}", w.name).into()
                }
                None => "(closed)".to_string().into(),
            },
            Tab::Browser(id) => self.browser.tab_title(*id).into(),
            Tab::McpManager => "MCP Servers".into(),
            Tab::SkillsManager => "Skills".into(),
            Tab::AgentManager => "Agents".into(),
            Tab::Pack => crate::pack::DEN_LABEL.into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Dashboard => views::dashboard::render(
                ui,
                self.dashboard,
                self.sup,
                self.browser,
                self.accents,
                self.actions,
            ),
            Tab::Chat(id) => match self.sup.get_mut(*id) {
                Some(ws) => ws.render_chat(ui, self.browser, self.composer_style),
                None => closed_placeholder(ui),
            },
            Tab::Browser(id) => self.browser.render_tab(ui, *id),
            Tab::McpManager => views::mcp_manager::render(ui, self.sup, self.mcp),
            Tab::SkillsManager => views::skills_manager::render(ui, self.sup, self.skills),
            Tab::AgentManager => views::agent_manager::render(ui, self.sup, self.agents),
            Tab::Pack => {
                // Attach the local puppy's name to our pack presence (prefer a
                // workspace that has learned its real name over the default).
                let puppy = self
                    .sup
                    .iter()
                    .map(|w| w.puppy_name.clone())
                    .find(|p| !p.is_empty() && p != "Puppy")
                    .unwrap_or_default();
                views::pack_panel::render(ui, self.pack, &puppy)
            }
        }
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        match tab {
            Tab::Dashboard => egui::Id::new("tab-dashboard"),
            Tab::Chat(id) => egui::Id::new(("tab-chat", id.0)),
            Tab::Browser(id) => egui::Id::new(("tab-browser", id.0)),
            Tab::McpManager => egui::Id::new("tab-mcp-manager"),
            Tab::SkillsManager => egui::Id::new("tab-skills-manager"),
            Tab::AgentManager => egui::Id::new("tab-agent-manager"),
            Tab::Pack => egui::Id::new("tab-pack"),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Dashboard)
    }

    fn on_close(&mut self, tab: &mut Tab) -> OnCloseResponse {
        // Closing a chat tab closes the whole workspace; closing a Diffs tab
        // just removes that view.
        match tab {
            Tab::Chat(id) => self.actions.push(ShellAction::Close(*id)),
            Tab::Browser(id) => self.browser.close_tab(*id),
            Tab::Dashboard
            | Tab::McpManager
            | Tab::SkillsManager
            | Tab::AgentManager
            | Tab::Pack => {}
        }
        OnCloseResponse::Close
    }
}

fn closed_placeholder(ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        ui.weak("workspace closed");
    });
}
