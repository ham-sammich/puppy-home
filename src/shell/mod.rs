//! The dockable shell: tab descriptors, the `TabViewer`, and deferred actions.

use std::path::PathBuf;

use eframe::egui;
use egui_dock::TabViewer;
use egui_dock::widgets::tab_viewer::OnCloseResponse;

use crate::supervisor::Supervisor;
use crate::views;
use crate::workspace::{InstanceStatus, WorkspaceId};

/// A tab is a lightweight descriptor — heavy state lives in the supervisor.
#[derive(Clone, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Chat(WorkspaceId),
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
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Dashboard => views::dashboard::render(ui, self.sup, self.actions),
            Tab::Chat(id) => match self.sup.get_mut(*id) {
                Some(ws) => ws.render_chat(ui),
                None => closed_placeholder(ui),
            },
        }
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        match tab {
            Tab::Dashboard => egui::Id::new("tab-dashboard"),
            Tab::Chat(id) => egui::Id::new(("tab-chat", id.0)),
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        !matches!(tab, Tab::Dashboard)
    }

    fn on_close(&mut self, tab: &mut Tab) -> OnCloseResponse {
        // Closing a chat tab closes the whole workspace; closing a Diffs tab
        // just removes that view.
        if let Tab::Chat(id) = tab {
            self.actions.push(ShellAction::Close(*id));
        }
        OnCloseResponse::Close
    }
}

fn closed_placeholder(ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        ui.weak("workspace closed");
    });
}
