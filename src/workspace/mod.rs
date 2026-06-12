//! A workspace: one Code Puppy sidecar (scoped to a folder) plus its chat UI.
//!
//! Each workspace owns its backend handle and event receiver, folds incoming
//! [`UiEvent`]s into its state (including a derived [`InstanceStatus`] for the
//! dashboard), and renders its own chat tab. Multiple workspaces run side by
//! side under the supervisor.
//!
//! The implementation is split by responsibility across submodules; all the
//! `impl Workspace` blocks operate on the one struct defined here.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui_commonmark::CommonMarkCache;

use crate::backend::{AgentInfo, CodePuppy, CommandInfo, CompletionItem, ModelInfo, UiEvent};

mod ask;
mod chat;
mod chat_body;
pub(crate) mod clipboard;
mod composer;
mod diff;
mod dock;
mod editor;
mod editor_area;
mod events;
mod file_picker;
pub(crate) mod fs;
mod git_creds;
mod git_graph;
mod git_graph_view;
mod git_view;
mod pending_prompt;
mod render;
mod sessions;
mod state;
mod tree_ops;
mod view;

pub use state::{InstanceStatus, SPARK_SAMPLES, SparkRing};

use ask::AskState;
use diff::DiffRecord;
use state::{EditorItem, Entry, FileBuffer, GitView, Pending, PendingKind};

/// Stable, never-reused identity for a workspace.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorkspaceId(pub u64);

/// An image pasted into the composer, attached to the next prompt. Holds the
/// PNG bytes (base64, sent to the sidecar as a `BinaryContent`) plus a GPU
/// texture for the in-composer thumbnail.
pub(crate) struct PendingImage {
    pub png_base64: String,
    pub size: [usize; 2],
    pub texture: eframe::egui::TextureHandle,
}

pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub name: String,
    pub backend: Option<CodePuppy>,
    rx: Receiver<UiEvent>,

    // chat state
    transcript: Vec<Entry>,
    /// Count of oldest entries dropped by the transcript ring-buffer cap
    /// (surfaced as a "trimmed" banner so the cap is visible).
    transcript_collapsed: usize,
    /// Images pasted into the composer, attached to (and cleared by) the next
    /// prompt submit.
    pending_images: Vec<PendingImage>,
    logs: Vec<String>,
    commands: Vec<CommandInfo>,
    agents: Vec<AgentInfo>,
    models: Vec<ModelInfo>,
    input: String,
    pending: Option<Pending>,
    pending_ask: Option<AskState>,
    show_logs: bool,
    request_input_focus: bool,

    // inline completion
    completions: Vec<CompletionItem>,
    comp_selected: usize,
    comp_visible: bool,
    comp_request_id: u64,
    last_query: String,

    // shell-style input history (Up/Down to recall sent messages)
    input_history: Vec<String>,
    history_pos: Option<usize>,
    history_stash: String,

    // identity / status
    pub agent: String,
    pub model: String,
    /// Agent/model/session to re-apply once the sidecar is ready (restored session).
    restore_agent: Option<String>,
    restore_model: Option<String>,
    restore_session: Option<String>,
    /// The Code Puppy autosave session this workspace is currently tied to.
    pub autosave: String,
    /// The puppy's name (global Code Puppy config) + a rename buffer.
    pub puppy_name: String,
    owner_name: String,
    name_edit: Option<String>,
    // Session browser (interactive picker) + read-only conversation preview.
    sessions: Vec<crate::backend::SessionInfo>,
    sessions_current: String,
    show_sessions: bool,
    selected_session: Option<String>,
    session_preview: Option<(String, Vec<crate::backend::SessionEntry>)>,
    preview_cache: CommonMarkCache,
    sessions_filter: String,
    pub cp_version: String,
    pub cwd: String,
    pub status: InstanceStatus,
    ready: bool,
    running: bool,
    pub last_activity: Instant,
    pub turn_started: Option<Instant>,
    pub current_tool: Option<String>,
    pub tool_calls: u64,
    pub status_line: String,
    paused: bool,
    steer_queue_mode: bool,
    // live run metrics (from the `status` op, polled while busy)
    pub run_stats: String,
    pub token_rate: f64,
    pub sub_agents: Vec<crate::backend::SubAgentInfo>,
    /// The last user prompt sent this session (for the redesign's agent cards).
    pub last_prompt: String,
    /// Steering messages queued sidecar-side, waiting to be drained.
    pub queued_steers: u64,
    /// Cumulative provider-reported tokens across all turns this session.
    pub total_tokens: u64,
    /// Cumulative $ cost; `None` until Code Puppy exposes a cost ledger.
    pub cost: Option<f64>,
    /// Recent tok/s samples (one per status poll) for this card's sparkline.
    sparks: state::SparkRing,
    /// MCP server catalog (global Code Puppy config, fetched via this
    /// workspace's sidecar). `None` until the first `mcp_servers` event.
    pub mcp_servers: Option<Vec<crate::backend::McpServerInfo>>,
    /// Bumped on every `mcp_servers` event so views can drop optimistic state.
    pub mcp_generation: u64,
    /// Skill catalog (global + project Code Puppy config, fetched via this
    /// workspace's sidecar). `None` until the first `skills` event.
    pub skills: Option<Vec<crate::backend::SkillInfo>>,
    /// Bumped on every `skills` event so views can drop optimistic state.
    pub skills_generation: u64,
    /// The most recent `skill_detail` answer (the Skills tab's detail pane).
    pub skill_detail: Option<crate::backend::SkillDetail>,
    /// Agent catalog (JSON-editable + built-in), fetched via this sidecar.
    /// `None` until the first `agent_configs` event.
    pub agent_configs: Option<Vec<crate::backend::AgentConfigInfo>>,
    /// Bumped on every `agent_configs` event so views drop optimistic state.
    pub agent_configs_generation: u64,
    /// Available tool names for the visual builder (from `agent_configs`).
    pub agent_tool_catalog: Vec<String>,
    /// Available MCP server names for agent bindings (from `agent_configs`).
    pub agent_mcp_catalog: Vec<String>,
    /// The most recent `agent_config` answer (the Agent tab's detail pane).
    pub agent_config_detail: Option<crate::backend::AgentConfigDetail>,
    status_req_at: Instant,
    md_cache: CommonMarkCache,
    // changes: Code-Puppy-reported diffs (fallback for non-git folders) + the
    // currently displayed diff.
    diffs: Vec<DiffRecord>,
    current_diff: Option<DiffRecord>,
    // git working-tree status (preferred when the folder is a repo)
    git_repo: bool,
    /// Git backend for this workspace. Local today; a future remote impl routes
    /// these over the sidecar protocol. The IDE calls `self.git.<op>()`.
    git: Arc<dyn crate::git::WorkspaceGit>,
    git_changes: Vec<crate::git::GitChange>,
    git_rx: Option<Receiver<Vec<crate::git::GitChange>>>,
    git_refresh_at: Instant,
    git_pending: bool,
    // IDE: file tree + open editor buffers + editor-area tabs
    /// Filesystem access for this workspace's files. Local today; a future
    /// remote impl routes these over the sidecar protocol. The tree + editor
    /// go through this instead of calling `std::fs` directly.
    fs: Arc<dyn fs::WorkspaceFs>,
    /// `Some("user@host")` when this workspace's sidecar runs on a remote host
    /// over SSH. The chat works today; the file tree + git stay local-only
    /// until the remote fs/git impls land, so we show a placeholder instead.
    remote_label: Option<String>,
    show_tree: bool,
    open_files: BTreeMap<PathBuf, FileBuffer>,
    editor_open: Vec<EditorItem>,
    editor_active: usize,
    /// Editor-area placement relative to chat (stacked vs side-by-side).
    editor_side: crate::workspace::state::EditorSide,
    /// Last CDP endpoint written to `.puppy/browser.json` (avoids rewriting).
    browser_cdp_written: Option<String>,
    /// A file/folder pending delete confirmation (from the tree context menu).
    pending_delete: Option<PathBuf>,
    /// Error from the most recent delete attempt (shown in the confirm modal).
    delete_error: Option<String>,
    /// A path being renamed via the tree context menu (modal).
    pending_rename: Option<crate::workspace::state::PendingRename>,
    /// A new file/folder being created via the tree context menu (modal).
    pending_new: Option<crate::workspace::state::PendingNew>,
    /// Open "add a file to the chat" browser, holding the directory it shows.
    file_browser: Option<PathBuf>,
    /// Render the full transcript instead of just the recent tail (opt-in via
    /// the "Show older" button; old entries are expensive to re-lay-out).
    transcript_show_all: bool,
    // Git view (Source Control page + commit/blame tabs)
    git_view: Option<GitView>,
    /// All-branches commits for the graph view (newest first, with parents/refs).
    git_graph_commits: Vec<crate::git::Commit>,
    /// Whether the Git page shows the GitKraken-style graph vs. the flat list.
    git_show_graph: bool,
    /// Active "create branch here" dialog (commit hash/short + typed name).
    git_branch_dialog: Option<git_graph::BranchDialog>,
    commit_msg: String,
    /// Height of the commit-message box (changed only by dragging its strip;
    /// never derived from layout, so it can't creep -- see render_git).
    commit_box_h: f32,
    git_action_msg: Option<(bool, String)>,
    /// Active credentials modal when a remote push/pull/fetch needs HTTPS auth.
    git_creds: Option<crate::workspace::state::GitCredsPrompt>,
    commit_view: Option<(String, DiffRecord)>,
    blame_cache: HashMap<PathBuf, Vec<crate::git::BlameLine>>,
    /// Files currently showing the inline blame gutter (toggled per file).
    blame_files: std::collections::HashSet<PathBuf>,
    // Embedded shell terminal (lazy-spawned), shown in the chat area when on.
    terminal: Option<crate::terminal::Terminal>,
    show_terminal: bool,
}

impl Workspace {
    pub fn new(
        id: WorkspaceId,
        root: PathBuf,
        remote_label: Option<String>,
        fs: Arc<dyn fs::WorkspaceFs>,
        git: Arc<dyn crate::git::WorkspaceGit>,
        backend: CodePuppy,
        rx: Receiver<UiEvent>,
    ) -> Self {
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let is_git_repo = git.is_repo();
        Workspace {
            id,
            root,
            name,
            backend: Some(backend),
            rx,
            transcript: Vec::new(),
            transcript_collapsed: 0,
            pending_images: Vec::new(),
            logs: Vec::new(),
            commands: Vec::new(),
            agents: Vec::new(),
            models: Vec::new(),
            input: String::new(),
            pending: None,
            pending_ask: None,
            show_logs: false,
            request_input_focus: false,
            completions: Vec::new(),
            comp_selected: 0,
            comp_visible: false,
            comp_request_id: 0,
            last_query: String::new(),
            input_history: Vec::new(),
            history_pos: None,
            history_stash: String::new(),

            agent: String::new(),
            model: String::new(),
            restore_agent: None,
            restore_model: None,
            restore_session: None,
            autosave: String::new(),
            puppy_name: "Puppy".to_string(),
            owner_name: "Master".to_string(),
            name_edit: None,
            sessions: Vec::new(),
            sessions_current: String::new(),
            show_sessions: false,
            selected_session: None,
            session_preview: None,
            preview_cache: CommonMarkCache::default(),
            sessions_filter: String::new(),
            cp_version: String::new(),
            cwd: String::new(),
            status: InstanceStatus::Starting,
            ready: false,
            running: false,
            last_activity: Instant::now(),
            turn_started: None,
            current_tool: None,
            tool_calls: 0,
            status_line: "Starting Code Puppy…".to_string(),
            paused: false,
            steer_queue_mode: false,
            run_stats: String::new(),
            token_rate: 0.0,
            sub_agents: Vec::new(),
            last_prompt: String::new(),
            queued_steers: 0,
            total_tokens: 0,
            cost: None,
            sparks: state::SparkRing::new(state::SPARK_SAMPLES),
            mcp_servers: None,
            mcp_generation: 0,
            skills: None,
            skills_generation: 0,
            skill_detail: None,
            agent_configs: None,
            agent_configs_generation: 0,
            agent_tool_catalog: Vec::new(),
            agent_mcp_catalog: Vec::new(),
            agent_config_detail: None,
            status_req_at: Instant::now(),
            md_cache: CommonMarkCache::default(),
            diffs: Vec::new(),
            current_diff: None,
            git_repo: is_git_repo,
            git,
            fs,
            remote_label,
            git_changes: Vec::new(),
            git_rx: None,
            git_refresh_at: Instant::now(),
            git_pending: false,
            show_tree: true,
            open_files: BTreeMap::new(),
            editor_open: Vec::new(),
            editor_active: 0,
            editor_side: crate::workspace::state::EditorSide::default(),
            browser_cdp_written: None,
            pending_delete: None,
            delete_error: None,
            pending_rename: None,
            pending_new: None,
            file_browser: None,
            transcript_show_all: false,
            git_view: None,
            git_graph_commits: Vec::new(),
            git_show_graph: true,
            git_branch_dialog: None,
            commit_msg: String::new(),
            commit_box_h: 74.0,
            git_action_msg: None,
            git_creds: None,
            commit_view: None,
            blame_cache: HashMap::new(),
            blame_files: std::collections::HashSet::new(),
            terminal: None,
            show_terminal: false,
        }
    }

    /// Number of file changes recorded so far (for tab badges).
    /// Is this workspace on a remote host (fs/git/sidecar over SSH)?
    pub fn is_remote(&self) -> bool {
        self.remote_label.is_some()
    }

    pub fn diff_count(&self) -> usize {
        self.diffs.len()
    }

    /// Whether the sidecar has announced `ready` (and hasn't died since).
    pub fn is_ready(&self) -> bool {
        self.ready && self.status != InstanceStatus::Dead
    }

    /// Whether the running turn is held at the pause gate. Mirrors the private
    /// flag for views outside this module. (The dashboard reads the derived
    /// `InstanceStatus::Paused` instead; this stays for the chat redesign.)
    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Recent tok/s samples, oldest → newest (this card's sparkline data).
    pub fn spark_history(&self) -> &[f32] {
        self.sparks.samples()
    }

    /// The model catalog the sidecar announced (the card's model popover).
    pub fn model_catalog(&self) -> &[ModelInfo] {
        &self.models
    }

    /// The outstanding input/confirm/select request, if any (frontends
    /// render it; answers go through [`Self::pending_choose`] /
    /// [`Self::pending_answer_text`]).
    #[allow(dead_code)] // the egui modal drives answer_pending() directly
    pub(crate) fn pending_request(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// Answer a confirm/select request by picking option `i`.
    #[allow(dead_code)] // the egui modal drives answer_pending() directly
    pub(crate) fn pending_choose(&mut self, i: usize) {
        if let Some(p) = self.pending.as_mut() {
            p.selection = i;
            self.answer_pending();
        }
    }

    /// Answer an input request with typed text.
    #[allow(dead_code)] // the egui modal drives answer_pending() directly
    pub(crate) fn pending_answer_text(&mut self, text: &str) {
        if let Some(p) = self.pending.as_mut() {
            p.text = text.to_string();
            self.answer_pending();
        }
    }

    /// The question text of an outstanding interactive request, if any (shown
    /// on waiting cards + the attention banner).
    pub fn pending_question(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| match &p.kind {
            PendingKind::Input { prompt, .. } => prompt.as_str(),
            PendingKind::Confirm { title, .. } => title.as_str(),
            PendingKind::Select { prompt, .. } => prompt.as_str(),
        })
    }

    /// Session diff totals: (+lines, −lines) across recorded diff records
    /// (feeds the den roster's +added/−removed counters).
    pub fn diff_totals(&self) -> (u64, u64) {
        self.diffs.iter().fold((0, 0), |(adds, dels), d| {
            (adds + d.adds as u64, dels + d.dels as u64)
        })
    }

    /// The file most recently touched by a recorded diff, if any.
    pub fn last_file(&self) -> Option<&str> {
        self.diffs.last().map(|d| d.path.as_str())
    }

    /// Resolve a (possibly relative) diff path against the workspace root.
    pub(crate) fn abs_path(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            self.root.join(pb)
        }
    }

    /// Queue an agent/model/session to re-apply once the sidecar is ready (restore).
    pub fn set_restore(
        &mut self,
        agent: Option<String>,
        model: Option<String>,
        session: Option<String>,
    ) {
        self.restore_agent = agent.filter(|s| !s.is_empty());
        self.restore_model = model.filter(|s| !s.is_empty());
        self.restore_session = session.filter(|s| !s.is_empty());
    }

    /// Relaunch a crashed/exited sidecar for this workspace and re-attach the
    /// conversation. The fresh sidecar's `Ready` re-applies agent/model/session
    /// through the existing restore path, so the chat picks up where it died.
    pub fn restart(&mut self, waker: std::sync::Arc<dyn crate::waker::UiWaker>) {
        match CodePuppy::spawn(waker, Some(&self.root)) {
            Ok((backend, rx)) => {
                self.rx = rx;
                self.backend = Some(backend);
                self.begin_restart();
            }
            Err(e) => self
                .transcript
                .push(Entry::Note(format!("Restart failed: {e}"))),
        }
    }

    /// Reset live state and arm the restore path after a successful re-spawn.
    /// Separated from `restart` (which does process I/O) for testability.
    fn begin_restart(&mut self) {
        let resume = restart_resume_target(&self.autosave).map(str::to_string);
        self.set_restore(Some(self.agent.clone()), Some(self.model.clone()), resume);
        self.ready = false;
        self.running = false;
        self.paused = false;
        self.current_tool = None;
        self.pending = None;
        self.pending_ask = None;
        // Bypass set_status's `Dead` guard: a restart legitimately revives us.
        self.status = InstanceStatus::Idle;
        self.status_line = "Restarting Code Puppy...".to_string();
        self.transcript.push(Entry::Note(
            "Code Puppy stopped - restarting and restoring the session.".into(),
        ));
        self.enforce_transcript_cap();
    }

    /// Drain this workspace's event stream into state (called by the supervisor).
    pub fn pump(&mut self) {
        let events: Vec<UiEvent> = self.rx.try_iter().collect();
        for event in events {
            self.apply_event(event);
        }
        self.poll_status();
        if let Some(term) = &mut self.terminal {
            term.pump();
        }
    }

    /// While a turn is running, periodically ask the sidecar for a live metrics
    /// snapshot (conversation stats + concurrent sub-agents) for the dashboard.
    pub(crate) fn poll_status(&mut self) {
        if !self.running {
            return;
        }
        let now = Instant::now();
        if now < self.status_req_at {
            return;
        }
        if let Some(backend) = &self.backend {
            backend.request_status();
        }
        self.status_req_at = now + std::time::Duration::from_millis(1200);
    }
}

/// Remove the transient browser CDP breadcrumb (`<root>/.puppy/browser.json`),
/// its self-contained `.gitignore`, and the dir if now empty. Best-effort.
pub(crate) fn cleanup_puppy_browser(root: &std::path::Path) {
    let dir = root.join(".puppy");
    let _ = std::fs::remove_file(dir.join("browser.json"));
    let _ = std::fs::remove_file(dir.join(".gitignore"));
    let _ = std::fs::remove_dir(&dir); // only succeeds if empty — leaves user files
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // Closing the workspace (or app exit) removes its CDP breadcrumb so it
        // never lingers pointing at a dead port.
        if self.browser_cdp_written.is_some() {
            cleanup_puppy_browser(&self.root);
        }
    }
}

/// Max transcript entries kept live. Immediate-mode rendering re-lays out every
/// entry each frame, so this bounds per-frame work on a chatty turn.
const MAX_TRANSCRIPT: usize = 1500;

/// Drop the oldest entries so `transcript` holds at most `max`; returns how many
/// were removed (accumulated into the "collapsed" count for the banner).
fn trim_transcript(transcript: &mut Vec<Entry>, max: usize) -> usize {
    if transcript.len() <= max {
        return 0;
    }
    let drop = transcript.len() - max;
    transcript.drain(0..drop);
    drop
}

/// The autosave session to reload after a sidecar restart, if any. An empty or
/// whitespace-only name means "fresh session" - nothing to re-attach.
fn restart_resume_target(autosave: &str) -> Option<&str> {
    let s = autosave.trim();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_target_skips_blank_autosave() {
        assert_eq!(
            restart_resume_target("auto_session_9"),
            Some("auto_session_9")
        );
        assert_eq!(restart_resume_target(""), None);
        assert_eq!(restart_resume_target("   "), None);
    }

    fn notes(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry::Note(i.to_string())).collect()
    }

    #[test]
    fn trim_is_noop_under_cap() {
        let mut t = notes(10);
        assert_eq!(trim_transcript(&mut t, 100), 0);
        assert_eq!(t.len(), 10);
    }

    #[test]
    fn trim_drops_oldest_and_reports_count() {
        let mut t = notes(10);
        let dropped = trim_transcript(&mut t, 6);
        assert_eq!(dropped, 4);
        assert_eq!(t.len(), 6);
        // Oldest (0..4) dropped; newest kept, order intact.
        match &t[0] {
            Entry::Note(s) => assert_eq!(s, "4"),
            _ => panic!("expected note"),
        }
        match t.last().unwrap() {
            Entry::Note(s) => assert_eq!(s, "9"),
            _ => panic!("expected note"),
        }
    }

    #[test]
    fn trim_steady_state_drops_one_at_a_time() {
        let mut t = notes(MAX_TRANSCRIPT);
        t.push(Entry::Note("new".into()));
        assert_eq!(trim_transcript(&mut t, MAX_TRANSCRIPT), 1);
        assert_eq!(t.len(), MAX_TRANSCRIPT);
    }
}
