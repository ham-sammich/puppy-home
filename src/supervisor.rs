//! Supervisor: owns and drives all open workspaces (one Code Puppy each).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use eframe::egui;

use crate::backend::{CodePuppy, UiEvent};
use crate::workspace::fs::{LocalFs, WorkspaceFs};
use crate::workspace::{InstanceStatus, Workspace, WorkspaceId};

pub struct Supervisor {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    next_id: u64,
    ctx: egui::Context,
}

impl Supervisor {
    pub fn new(ctx: egui::Context) -> Self {
        Supervisor {
            workspaces: BTreeMap::new(),
            next_id: 1,
            ctx,
        }
    }

    /// Open a folder as a new workspace: spawn a Code Puppy sidecar scoped to it.
    pub fn open(&mut self, root: PathBuf) -> Result<WorkspaceId, String> {
        let (backend, rx) = CodePuppy::spawn(self.ctx.clone(), Some(&root))?;
        Ok(self.adopt(root, None, Arc::new(LocalFs), backend, rx))
    }

    /// Adopt an already-spawned backend (used for remote workspaces, whose SSH
    /// connection is established off-thread). `remote_label` is `Some("user@host")`
    /// for a remote sidecar, `None` for local.
    pub fn adopt(
        &mut self,
        root: PathBuf,
        remote_label: Option<String>,
        fs: Arc<dyn WorkspaceFs>,
        backend: CodePuppy,
        rx: Receiver<UiEvent>,
    ) -> WorkspaceId {
        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        self.workspaces
            .insert(id, Workspace::new(id, root, remote_label, fs, backend, rx));
        id
    }

    /// Close a workspace (drops the handle → shuts down + kills the child).
    pub fn close(&mut self, id: WorkspaceId) {
        self.workspaces.remove(&id);
    }

    /// Fold each workspace's pending events into its state.
    pub fn drain(&mut self) {
        for ws in self.workspaces.values_mut() {
            ws.pump();
        }
    }

    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn get_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values()
    }

    pub fn len(&self) -> usize {
        self.workspaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }

    /// True while any workspace is mid-turn (drives elapsed-timer repaints).
    pub fn any_busy(&self) -> bool {
        self.workspaces
            .values()
            .any(|w| !matches!(w.status, InstanceStatus::Idle | InstanceStatus::Dead))
    }

    /// How many workspaces are blocked waiting for user input.
    pub fn waiting_count(&self) -> usize {
        self.workspaces
            .values()
            .filter(|w| w.status == InstanceStatus::WaitingForInput)
            .count()
    }
}
