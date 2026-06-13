//! Supervisor: owns and drives all open workspaces (one Code Puppy each).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::backend::{CodePuppy, UiEvent};
use crate::git::{LocalGit, WorkspaceGit};
use crate::waker::UiWaker;
use crate::workspace::fs::{CachedFs, LocalFs, WorkspaceFs};
use crate::workspace::{InstanceStatus, SPARK_SAMPLES, SparkRing, Workspace, WorkspaceId};

/// Minimum spacing between aggregate-throughput samples. `drain` runs every
/// frame, so this gate is what keeps sampling off the per-frame cost path.
const AGG_SAMPLE_EVERY: Duration = Duration::from_secs(1);

pub struct Supervisor {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    next_id: u64,
    /// Wakes the frontend when a backend thread has fresh events.
    waker: Arc<dyn UiWaker>,
    /// Recent fleet-wide tok/s samples (sum across busy workspaces).
    agg_sparks: SparkRing,
    /// When the aggregate was last sampled (`None` = never).
    agg_sampled_at: Option<Instant>,
}

impl Supervisor {
    pub fn new(waker: Arc<dyn UiWaker>) -> Self {
        Supervisor {
            workspaces: BTreeMap::new(),
            next_id: 1,
            waker,
            agg_sparks: SparkRing::new(SPARK_SAMPLES),
            agg_sampled_at: None,
        }
    }

    /// Open a folder as a new workspace: spawn a Code Puppy sidecar scoped to it.
    pub fn open(&mut self, root: PathBuf) -> Result<WorkspaceId, String> {
        let (backend, rx) = CodePuppy::spawn(self.waker.clone(), Some(&root))?;
        let git: Arc<dyn WorkspaceGit> = Arc::new(LocalGit::new(root.clone()));
        // TTL-cached so the per-frame tree doesn't enumerate NTFS every frame.
        let fs: Arc<dyn WorkspaceFs> = Arc::new(CachedFs::new(LocalFs));
        Ok(self.adopt(root, None, fs, git, backend, rx))
    }

    /// Adopt an already-spawned backend (used for remote workspaces, whose SSH
    /// connection is established off-thread). `remote` carries the label AND
    /// the full ssh target for a remote sidecar, `None` for local.
    pub fn adopt(
        &mut self,
        root: PathBuf,
        remote: Option<crate::workspace::RemoteInfo>,
        fs: Arc<dyn WorkspaceFs>,
        git: Arc<dyn WorkspaceGit>,
        backend: CodePuppy,
        rx: Receiver<UiEvent>,
    ) -> WorkspaceId {
        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        self.workspaces
            .insert(id, Workspace::new(id, root, remote, fs, git, backend, rx));
        id
    }

    /// Close a workspace (drops the handle → shuts down + kills the child).
    pub fn close(&mut self, id: WorkspaceId) {
        self.workspaces.remove(&id);
    }

    /// Relaunch a dead workspace's sidecar (the card's "Restart" action).
    pub fn restart(&mut self, id: WorkspaceId) {
        let waker = self.waker.clone();
        if let Some(ws) = self.workspaces.get_mut(&id) {
            ws.restart(waker);
        }
    }

    /// Fold each workspace's pending events into its state.
    pub fn drain(&mut self) {
        for ws in self.workspaces.values_mut() {
            ws.pump();
        }
        self.sample_aggregate(Instant::now());
    }

    /// Record one fleet-wide tok/s sample (sum across busy workspaces), at
    /// most once per [`AGG_SAMPLE_EVERY`]. `now` is a parameter so tests can
    /// drive the cadence without sleeping.
    fn sample_aggregate(&mut self, now: Instant) {
        if let Some(last) = self.agg_sampled_at
            && now.duration_since(last) < AGG_SAMPLE_EVERY
        {
            return;
        }
        self.agg_sampled_at = Some(now);
        // Idle/dead workspaces hold their LAST observed rate — summing those
        // would overstate live throughput, so only busy ones count.
        let total: f32 = self
            .workspaces
            .values()
            .filter(|w| !matches!(w.status, InstanceStatus::Idle | InstanceStatus::Dead))
            .map(|w| w.token_rate as f32)
            .sum();
        self.agg_sparks.push(total);
    }

    /// Fleet-wide tok/s samples, oldest → newest (the Command Center spark).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn aggregate_sparks(&self) -> &[f32] {
        self.agg_sparks.samples()
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

    /// Workspaces the user should SEE: skips hidden/ephemeral sessions like
    /// the Agent Creator chat (which is still pumped via `drain`, just not
    /// surfaced on the dashboard, persisted, or counted) (F8).
    pub fn iter_visible(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values().filter(|w| !w.ephemeral)
    }

    /// Count of visible (non-ephemeral) workspaces.
    pub fn visible_len(&self) -> usize {
        self.workspaces.values().filter(|w| !w.ephemeral).count()
    }

    /// True when there are no VISIBLE workspaces (an ephemeral creator
    /// session alone still shows the empty dashboard).
    pub fn visible_is_empty(&self) -> bool {
        !self.workspaces.values().any(|w| !w.ephemeral)
    }

    /// Open a hidden, throwaway session (Agent Creator) rooted at `root`.
    pub fn open_ephemeral(&mut self, root: PathBuf) -> Result<WorkspaceId, String> {
        let id = self.open(root)?;
        if let Some(ws) = self.workspaces.get_mut(&id) {
            ws.ephemeral = true;
        }
        Ok(id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waker::NoopWaker;

    #[test]
    fn aggregate_sampling_respects_cadence() {
        let mut sup = Supervisor::new(Arc::new(NoopWaker));
        let t0 = Instant::now();
        // First sample always records (nothing to gate against yet).
        sup.sample_aggregate(t0);
        assert_eq!(sup.aggregate_sparks().len(), 1);
        // Inside the gate window: dropped, not queued.
        sup.sample_aggregate(t0 + Duration::from_millis(300));
        sup.sample_aggregate(t0 + Duration::from_millis(900));
        assert_eq!(sup.aggregate_sparks().len(), 1);
        // Past the gate: records, and the gate re-anchors on THIS sample.
        sup.sample_aggregate(t0 + Duration::from_millis(1100));
        assert_eq!(sup.aggregate_sparks().len(), 2);
        sup.sample_aggregate(t0 + Duration::from_millis(1900));
        assert_eq!(sup.aggregate_sparks().len(), 2);
        sup.sample_aggregate(t0 + Duration::from_millis(2200));
        assert_eq!(sup.aggregate_sparks().len(), 3);
        // No workspaces open -> the aggregate is a flat zero line.
        assert!(sup.aggregate_sparks().iter().all(|&v| v == 0.0));
    }
}
