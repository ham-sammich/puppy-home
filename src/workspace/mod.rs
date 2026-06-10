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
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui_commonmark::CommonMarkCache;

use crate::backend::{
    AgentInfo, BackendMessage, CodePuppy, CommandInfo, CompletionItem, ModelInfo, UiEvent,
};

mod ask;
mod chat;
mod clipboard;
mod composer;
mod diff;
mod editor;
mod git_graph;
mod git_graph_view;
mod git_view;
mod render;
mod sessions;
mod state;
mod view;

pub use state::InstanceStatus;

use ask::AskState;
use diff::{DiffRecord, parse_diff};
use render::short_session;
use state::{EditorItem, Entry, FileBuffer, GitView, Pending, parse_pending, tool_label};

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
    status_req_at: Instant,
    md_cache: CommonMarkCache,
    // changes: Code-Puppy-reported diffs (fallback for non-git folders) + the
    // currently displayed diff.
    diffs: Vec<DiffRecord>,
    current_diff: Option<DiffRecord>,
    // git working-tree status (preferred when the folder is a repo)
    git_repo: bool,
    git_changes: Vec<crate::git::GitChange>,
    git_rx: Option<Receiver<Vec<crate::git::GitChange>>>,
    git_refresh_at: Instant,
    git_pending: bool,
    // IDE: file tree + open editor buffers + editor-area tabs
    show_tree: bool,
    open_files: BTreeMap<PathBuf, FileBuffer>,
    editor_open: Vec<EditorItem>,
    editor_active: usize,
    /// Editor-area placement relative to chat (stacked vs side-by-side).
    editor_side: crate::workspace::state::EditorSide,
    /// Last CDP endpoint written to `.puppy/browser.json` (avoids rewriting).
    browser_cdp_written: Option<String>,
    // Git view (Source Control page + commit/blame tabs)
    git_view: Option<GitView>,
    /// All-branches commits for the graph view (newest first, with parents/refs).
    git_graph_commits: Vec<crate::git::Commit>,
    /// Whether the Git page shows the GitKraken-style graph vs. the flat list.
    git_show_graph: bool,
    /// Active "create branch here" dialog (commit hash/short + typed name).
    git_branch_dialog: Option<git_graph::BranchDialog>,
    commit_msg: String,
    git_action_msg: Option<(bool, String)>,
    commit_view: Option<(String, DiffRecord)>,
    blame_cache: HashMap<PathBuf, Vec<crate::git::BlameLine>>,
    /// Files currently showing the inline blame gutter (toggled per file).
    blame_files: std::collections::HashSet<PathBuf>,
    // Embedded shell terminal (lazy-spawned), shown in the chat area when on.
    terminal: Option<crate::terminal::Terminal>,
    show_terminal: bool,
}

impl Workspace {
    pub fn new(id: WorkspaceId, root: PathBuf, backend: CodePuppy, rx: Receiver<UiEvent>) -> Self {
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        let is_git_repo = crate::git::is_repo(&root);
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
            status_req_at: Instant::now(),
            md_cache: CommonMarkCache::default(),
            diffs: Vec::new(),
            current_diff: None,
            git_repo: is_git_repo,
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
            git_view: None,
            git_graph_commits: Vec::new(),
            git_show_graph: true,
            git_branch_dialog: None,
            commit_msg: String::new(),
            git_action_msg: None,
            commit_view: None,
            blame_cache: HashMap::new(),
            blame_files: std::collections::HashSet::new(),
            terminal: None,
            show_terminal: false,
        }
    }

    /// Number of file changes recorded so far (for tab badges).
    pub fn diff_count(&self) -> usize {
        self.diffs.len()
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
    pub fn restart(&mut self, ctx: &eframe::egui::Context) {
        match CodePuppy::spawn(ctx.clone(), Some(&self.root)) {
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

    pub(crate) fn apply_event(&mut self, event: UiEvent) {
        self.last_activity = Instant::now();
        match event {
            UiEvent::Ready {
                agent,
                model,
                cp_version,
                cwd,
                autosave,
                puppy_name,
                owner_name,
            } => {
                self.ready = true;
                self.agent = agent;
                self.model = model;
                self.cp_version = cp_version;
                self.cwd = cwd;
                self.autosave = autosave;
                if !puppy_name.is_empty() {
                    self.puppy_name = puppy_name;
                }
                if !owner_name.is_empty() {
                    self.owner_name = owner_name;
                }
                if self.status != InstanceStatus::Dead {
                    self.set_status(InstanceStatus::Idle);
                }
                self.status_line = format!("Ready · {} · {}", self.agent, self.model);
                // Re-apply a restored session's agent/model/conversation (once).
                // Order matters: agent/model first (a reload clears history), then
                // the session load, so the restored conversation survives.
                if let Some(backend) = &self.backend {
                    if let Some(a) = self.restore_agent.take()
                        && a != self.agent
                    {
                        backend.set_agent(&a);
                    }
                    if let Some(m) = self.restore_model.take()
                        && m != self.model
                    {
                        backend.set_model(&m);
                    }
                    if let Some(s) = self.restore_session.take()
                        && !s.is_empty()
                        && s != self.autosave
                    {
                        backend.load_session(&s, "autosave");
                    }
                }
            }
            UiEvent::Message(msg) => self.on_message(msg),
            UiEvent::Commands(items) => self.commands = items,
            UiEvent::Agents(items) => {
                if let Some(cur) = items.iter().find(|a| a.current) {
                    self.agent = cur.name.clone();
                }
                self.agents = items;
            }
            UiEvent::Models(items) => {
                if let Some(cur) = items.iter().find(|m| m.current) {
                    self.model = cur.name.clone();
                }
                self.models = items;
            }
            UiEvent::Completions { id, items } => {
                if id == self.comp_request_id {
                    self.completions = items;
                    self.comp_selected = 0;
                    self.comp_visible = !self.completions.is_empty();
                }
            }
            UiEvent::Ask { id, questions } => {
                let headers: Vec<String> = questions.iter().map(|q| q.header.clone()).collect();
                self.transcript
                    .push(Entry::Note(format!("🐶 asked: {}", headers.join(", "))));
                self.pending_ask = Some(AskState::from(id, questions));
                self.set_status(InstanceStatus::WaitingForInput);
            }
            UiEvent::Result { output, .. } => {
                self.running = false;
                self.turn_started = None;
                self.current_tool = None;
                self.sub_agents.clear();
                self.paused = false;
                self.collapse_thinking();
                self.set_status(InstanceStatus::Idle);
                self.transcript.push(Entry::Agent(output));
            }
            UiEvent::CommandDone { handled, .. } => {
                self.running = false;
                self.turn_started = None;
                self.current_tool = None;
                self.sub_agents.clear();
                self.paused = false;
                self.collapse_thinking();
                self.set_status(InstanceStatus::Idle);
                if !handled {
                    self.transcript
                        .push(Entry::Note("command not recognized".to_string()));
                }
            }
            UiEvent::Error { message, .. } => {
                self.running = false;
                self.turn_started = None;
                self.current_tool = None;
                self.sub_agents.clear();
                self.paused = false;
                self.collapse_thinking();
                self.set_status(InstanceStatus::Idle);
                self.transcript.push(Entry::Error(message));
            }
            UiEvent::Log(line) => self.logs.push(line),
            UiEvent::Status {
                stats,
                token_rate,
                sub_agents,
            } => {
                self.run_stats = stats;
                self.token_rate = token_rate;
                self.sub_agents = sub_agents;
            }
            UiEvent::Paused(paused) => self.paused = paused,
            UiEvent::Sessions {
                items,
                current,
                open,
            } => {
                self.sessions = items;
                self.sessions_current = current;
                if open {
                    self.show_sessions = true;
                }
            }
            UiEvent::SessionLoaded {
                name,
                messages,
                entries,
            } => {
                self.transcript.clear();
                self.transcript_collapsed = 0;
                self.transcript.push(Entry::Note(format!(
                    "⟲ Resumed session {} ({messages} messages)",
                    short_session(&name)
                )));
                for e in entries {
                    if e.role == "user" {
                        self.transcript.push(Entry::User(e.text));
                    } else {
                        self.transcript.push(Entry::Agent(e.text));
                    }
                }
                self.show_sessions = false;
            }
            UiEvent::SessionPreview { name, entries, .. } => {
                self.session_preview = Some((name, entries));
            }
            UiEvent::Exited { code } => {
                self.ready = false;
                self.running = false;
                self.pending_ask = None;
                self.set_status(InstanceStatus::Dead);
                self.backend = None;
                self.status_line = match code {
                    Some(c) => format!("Code Puppy exited (code {c})"),
                    None => "Code Puppy process ended".to_string(),
                };
                self.transcript.push(Entry::Note(self.status_line.clone()));
            }
        }
        self.enforce_transcript_cap();
    }

    /// Drop the oldest transcript entries past the ring-buffer cap. Immediate-mode
    /// rendering re-lays out every entry each frame, so an unbounded log would
    /// lock the UI on a chatty turn; keep the last `MAX_TRANSCRIPT`, count the rest.
    pub(crate) fn enforce_transcript_cap(&mut self) {
        self.transcript_collapsed += trim_transcript(&mut self.transcript, MAX_TRANSCRIPT);
    }

    pub(crate) fn set_status(&mut self, status: InstanceStatus) {
        if self.status != InstanceStatus::Dead {
            self.status = status;
        }
    }

    pub(crate) fn on_message(&mut self, msg: BackendMessage) {
        // Noise we never show in the chat.
        if msg.kind == "SpinnerControl" || msg.kind == "FileContentMessage" {
            return;
        }
        // Streamed reasoning: coalesce consecutive chunks into one live block so
        // a watching user can read the agent's thoughts and pause/steer.
        if msg.kind == "agent_reasoning" {
            self.current_tool = None;
            self.set_status(InstanceStatus::Thinking);
            if let Some(Entry::Thinking { text, .. }) = self.transcript.last_mut() {
                text.push_str(&msg.text);
            } else {
                self.transcript.push(Entry::Thinking {
                    text: msg.text,
                    collapse: std::cell::Cell::new(false),
                });
            }
            return;
        }
        if msg.kind == "DiffMessage" {
            if let Some(record) = parse_diff(&msg) {
                // The AI just wrote this file — refresh an open editor buffer so
                // it shows the new content (unless the user has unsaved edits).
                let abs = self.abs_path(&record.path);
                if let Some(buf) = self.open_files.get_mut(&abs)
                    && !buf.dirty
                    && let Ok(content) = std::fs::read_to_string(&abs)
                {
                    buf.content = content;
                    buf.load_error = None;
                }
                self.diffs.push(record);
            }
            // Refresh git status immediately so the change shows in the panel.
            self.git_refresh_at = Instant::now();
            self.current_tool = Some("edit".to_string());
            self.tool_calls += 1;
            self.set_status(InstanceStatus::ToolCalling);
            return;
        }
        match msg.category.as_str() {
            "user_interaction" => {
                if let Some(p) = parse_pending(&msg) {
                    self.pending = Some(p);
                }
                self.set_status(InstanceStatus::WaitingForInput);
            }
            "tool_output" => {
                self.current_tool = Some(tool_label(&msg.kind));
                self.tool_calls += 1;
                self.set_status(InstanceStatus::ToolCalling);
            }
            "agent" => {
                self.current_tool = None;
                self.set_status(InstanceStatus::Thinking);
            }
            _ => {}
        }
        self.transcript.push(Entry::Message(msg));
    }

    /// Fold all streamed thinking blocks (called when a turn completes).
    pub(crate) fn collapse_thinking(&self) {
        for entry in &self.transcript {
            if let Entry::Thinking { collapse, .. } = entry {
                collapse.set(true);
            }
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
