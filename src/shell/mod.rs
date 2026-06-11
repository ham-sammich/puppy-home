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
#[derive(Clone, PartialEq, Eq)]
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
}

/// Structural changes requested during rendering, applied after the dock draws.
pub enum ShellAction {
    #[allow(dead_code)] // reserved action (matched in app.rs); not yet emitted
    OpenFolder(PathBuf),
    Close(WorkspaceId),
    FocusChat(WorkspaceId),
    /// Focus a workspace's chat tab and switch its editor to the Changes view.
    ShowChanges(WorkspaceId),
}

/// Transient `TabViewer` holding mutable access to app state for one frame.
pub struct Shell<'a> {
    pub sup: &'a mut Supervisor,
    pub browser: &'a mut BrowserManager,
    pub mcp: &'a mut views::mcp_manager::McpManagerView,
    pub skills: &'a mut views::skills_manager::SkillsManagerView,
    pub agents: &'a mut views::agent_manager::AgentManagerView,
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
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Dashboard => views::dashboard::render(ui, self.sup, self.browser, self.actions),
            Tab::Chat(id) => match self.sup.get_mut(*id) {
                Some(ws) => ws.render_chat(ui, self.browser),
                None => closed_placeholder(ui),
            },
            Tab::Browser(id) => self.browser.render_tab(ui, *id),
            Tab::McpManager => views::mcp_manager::render(ui, self.sup, self.mcp),
            Tab::SkillsManager => views::skills_manager::render(ui, self.sup, self.skills),
            Tab::AgentManager => views::agent_manager::render(ui, self.sup, self.agents),
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
            Tab::Dashboard | Tab::McpManager | Tab::SkillsManager | Tab::AgentManager => {}
        }
        OnCloseResponse::Close
    }
}

fn closed_placeholder(ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        ui.weak("workspace closed");
    });
}
