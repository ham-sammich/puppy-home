//! A workspace: one Code Puppy sidecar (scoped to a folder) plus its chat UI.
//!
//! Each workspace owns its backend handle and event receiver, folds incoming
//! [`UiEvent`]s into its state (including a derived [`InstanceStatus`] for the
//! dashboard), and renders its own chat tab. Multiple workspaces run side by
//! side under the supervisor.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::Value;

use crate::backend::{
    AgentInfo, AskAnswer, AskOption, AskQuestion, BackendMessage, CodePuppy, CommandInfo,
    CompletionItem, ModelInfo, UiEvent,
};

/// Stable, never-reused identity for a workspace.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorkspaceId(pub u64);

/// Derived lifecycle state of an instance (for the dashboard + status line).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Starting,
    Idle,
    Running,
    Thinking,
    ToolCalling,
    WaitingForInput,
    Dead,
}

impl InstanceStatus {
    pub fn label(self) -> &'static str {
        match self {
            InstanceStatus::Starting => "starting",
            InstanceStatus::Idle => "idle",
            InstanceStatus::Running => "running",
            InstanceStatus::Thinking => "thinking",
            InstanceStatus::ToolCalling => "tool",
            InstanceStatus::WaitingForInput => "waiting for input",
            InstanceStatus::Dead => "dead",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            InstanceStatus::Starting => egui::Color32::from_rgb(150, 150, 150),
            InstanceStatus::Idle => egui::Color32::from_rgb(110, 116, 128),
            InstanceStatus::Running => egui::Color32::from_rgb(90, 160, 255),
            InstanceStatus::Thinking => egui::Color32::from_rgb(116, 208, 216),
            InstanceStatus::ToolCalling => egui::Color32::from_rgb(232, 192, 106),
            InstanceStatus::WaitingForInput => egui::Color32::from_rgb(215, 156, 220),
            InstanceStatus::Dead => egui::Color32::from_rgb(240, 128, 128),
        }
    }
}

/// One rendered line in the transcript.
enum Entry {
    User(String),
    Agent(String),
    Message(BackendMessage),
    Note(String),
    Error(String),
}

/// An outstanding interactive request from the agent.
struct Pending {
    prompt_id: String,
    kind: PendingKind,
    text: String,
    selection: usize,
}

enum PendingKind {
    Input { prompt: String, password: bool },
    Confirm { title: String, description: String, options: Vec<String> },
    Select { prompt: String, options: Vec<String> },
}

/// One file change (from Code Puppy's `DiffMessage` or a git diff).
#[derive(Clone)]
struct DiffRecord {
    path: String,
    operation: String,
    adds: usize,
    dels: usize,
    lines: Vec<DiffLine>,
}

#[derive(Clone)]
struct DiffLine {
    kind: String, // "add" | "remove" | "context"
    content: String,
}

/// An open file's editable contents.
struct FileBuffer {
    content: String,
    dirty: bool,
    load_error: Option<String>,
    save_error: Option<String>,
}

/// A tab in the workspace's editor area (above the chat).
#[derive(Clone, PartialEq, Eq)]
enum EditorItem {
    Changes,
    File(PathBuf),
    /// The Source Control / Git page (branch, staging, history).
    Git,
    /// A single commit's patch (opened from the history list).
    Commit { hash: String, short: String, subject: String },
    /// Per-line blame for a file.
    Blame(PathBuf),
}

/// Cached snapshot for the Git page (refreshed on demand / after git actions).
struct GitView {
    branch: String,
    upstream: bool,
    ahead: usize,
    behind: usize,
    staged: Vec<crate::git::GitStatusEntry>,
    unstaged: Vec<crate::git::GitStatusEntry>,
    log: Vec<crate::git::Commit>,
}

/// Directories never shown in the file tree (noisy / huge).
const TREE_IGNORE: &[&str] = &[
    ".git", "target", "node_modules", "__pycache__", ".venv", "venv", ".mypy_cache",
    ".pytest_cache", "dist", "build", ".idea", ".vscode",
];

/// An outstanding `ask_user_question` request, rendered as a modal.
struct AskState {
    id: String,
    questions: Vec<AskQ>,
}

struct AskQ {
    header: String,
    question: String,
    multi_select: bool,
    options: Vec<AskOption>,
    selected: Vec<bool>,
    other: String,
}

impl AskState {
    fn from(id: String, questions: Vec<AskQuestion>) -> Self {
        let questions = questions
            .into_iter()
            .map(|q| {
                let selected = vec![false; q.options.len()];
                AskQ {
                    header: q.header,
                    question: q.question,
                    multi_select: q.multi_select,
                    options: q.options,
                    selected,
                    other: String::new(),
                }
            })
            .collect();
        AskState { id, questions }
    }
}

pub struct Workspace {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub name: String,
    pub backend: Option<CodePuppy>,
    rx: Receiver<UiEvent>,

    // chat state
    transcript: Vec<Entry>,
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
    // Git view (Source Control page + commit/blame tabs)
    git_view: Option<GitView>,
    commit_msg: String,
    git_action_msg: Option<(bool, String)>,
    commit_view: Option<(String, DiffRecord)>,
    blame_cache: HashMap<PathBuf, Vec<crate::git::BlameLine>>,
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
            git_view: None,
            commit_msg: String::new(),
            git_action_msg: None,
            commit_view: None,
            blame_cache: HashMap::new(),
        }
    }

    /// Number of file changes recorded so far (for tab badges).
    pub fn diff_count(&self) -> usize {
        self.diffs.len()
    }

    /// Resolve a (possibly relative) diff path against the workspace root.
    fn abs_path(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            self.root.join(pb)
        }
    }

    /// Map each changed file (absolute path) to its change marker, for inline
    /// tree badges. Uses git working-tree status when the folder is a repo,
    /// otherwise falls back to Code-Puppy-reported diffs.
    fn tree_markers(&self) -> HashMap<PathBuf, char> {
        let mut map = HashMap::new();
        if self.git_repo {
            for c in &self.git_changes {
                map.insert(self.root.join(&c.path), c.marker);
            }
        } else {
            for d in &self.diffs {
                map.insert(self.abs_path(&d.path), op_marker(&d.operation));
            }
        }
        map
    }

    /// Code-Puppy-reported changes (non-git fallback), newest first:
    /// (diff index, path, marker).
    fn diff_changed_files(&self) -> Vec<(usize, String, char)> {
        let mut latest: HashMap<&str, (usize, char)> = HashMap::new();
        for (i, d) in self.diffs.iter().enumerate() {
            latest.insert(d.path.as_str(), (i, op_marker(&d.operation)));
        }
        let mut out: Vec<(usize, String, char)> = latest
            .into_iter()
            .map(|(p, (i, m))| (i, p.to_string(), m))
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out
    }

    /// Poll/refresh git working-tree status (off the UI thread).
    fn poll_git(&mut self, ctx: &egui::Context) {
        if !self.git_repo {
            return;
        }
        if let Some(rx) = &self.git_rx {
            if let Ok(changes) = rx.try_recv() {
                self.git_changes = changes;
                self.git_pending = false;
                self.git_rx = None;
            }
        }
        if !self.git_pending && Instant::now() >= self.git_refresh_at {
            let root = self.root.clone();
            let ctx2 = ctx.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.git_rx = Some(rx);
            self.git_pending = true;
            self.git_refresh_at = Instant::now() + std::time::Duration::from_millis(2000);
            std::thread::spawn(move || {
                let _ = tx.send(crate::git::status(&root));
                ctx2.request_repaint();
            });
        }
        // Keep refreshing while this workspace is on screen.
        ctx.request_repaint_after(std::time::Duration::from_millis(1500));
    }

    /// Show the diff for a git-tracked change.
    fn load_git_diff(&mut self, path: &str, marker: char) {
        let (lines, adds, dels) = if marker == '?' {
            let content = crate::git::untracked_content(&self.root, path).unwrap_or_default();
            let lines: Vec<DiffLine> = content
                .lines()
                .map(|l| DiffLine { kind: "add".into(), content: l.to_string() })
                .collect();
            let n = lines.len();
            (lines, n, 0)
        } else {
            parse_unified(&crate::git::diff(&self.root, path))
        };
        let operation = match marker {
            '?' | 'A' => "create",
            'D' => "delete",
            _ => "modify",
        }
        .to_string();
        self.current_diff = Some(DiffRecord {
            path: path.to_string(),
            operation,
            adds,
            dels,
            lines,
        });
        self.show_changes();
    }

    /// Show the diff for a Code-Puppy-reported change (non-git fallback).
    fn load_diff_index(&mut self, idx: usize) {
        if let Some(d) = self.diffs.get(idx) {
            self.current_diff = Some(d.clone());
            self.show_changes();
        }
    }

    /// Load a file into an editable buffer (no-op if already open).
    pub fn open_file(&mut self, path: PathBuf) {
        if self.open_files.contains_key(&path) {
            return;
        }
        let buffer = match std::fs::read_to_string(&path) {
            Ok(content) => FileBuffer {
                content,
                dirty: false,
                load_error: None,
                save_error: None,
            },
            Err(e) => FileBuffer {
                content: String::new(),
                dirty: false,
                load_error: Some(e.to_string()),
                save_error: None,
            },
        };
        self.open_files.insert(path, buffer);
    }

    /// Whether an open file has unsaved edits (for the tab title marker).
    pub fn is_file_dirty(&self, path: &Path) -> bool {
        self.open_files.get(path).map(|b| b.dirty).unwrap_or(false)
    }

    /// Open (or focus) a file in the editor area.
    pub fn open_editor_file(&mut self, path: PathBuf) {
        self.open_file(path.clone());
        let item = EditorItem::File(path);
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    /// Open (or focus) the Changes (diff) tab in the editor area.
    pub fn show_changes(&mut self) {
        match self.editor_open.iter().position(|t| *t == EditorItem::Changes) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(EditorItem::Changes);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    fn close_editor(&mut self, index: usize) {
        if index >= self.editor_open.len() {
            return;
        }
        self.editor_open.remove(index);
        if self.editor_active >= self.editor_open.len() {
            self.editor_active = self.editor_open.len().saturating_sub(1);
        }
    }

    fn focus_or_open(&mut self, item: EditorItem) {
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    /// Rebuild the cached Git-page snapshot (branch, staging, recent history).
    fn refresh_git_view(&mut self) {
        let info = crate::git::head_info(&self.root);
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        for e in crate::git::status_full(&self.root) {
            if e.is_staged() {
                staged.push(e.clone());
            }
            if e.is_unstaged() {
                unstaged.push(e);
            }
        }
        self.git_view = Some(GitView {
            branch: info.branch,
            upstream: info.upstream,
            ahead: info.ahead,
            behind: info.behind,
            staged,
            unstaged,
            log: crate::git::log(&self.root, 50),
        });
    }

    /// Open (or focus) the Source Control / Git page.
    pub fn show_git(&mut self) {
        self.refresh_git_view();
        self.focus_or_open(EditorItem::Git);
    }

    /// Open (or focus) a single commit's patch.
    fn open_commit(&mut self, c: &crate::git::Commit) {
        let text = crate::git::show(&self.root, &c.hash);
        let (lines, adds, dels) = parse_unified(&text);
        self.commit_view = Some((
            c.hash.clone(),
            DiffRecord {
                path: format!("{} {}", c.short, c.subject),
                operation: "modify".to_string(),
                adds,
                dels,
                lines,
            },
        ));
        self.focus_or_open(EditorItem::Commit {
            hash: c.hash.clone(),
            short: c.short.clone(),
            subject: c.subject.clone(),
        });
    }

    /// Open (or focus) the blame view for a file.
    fn show_blame(&mut self, path: PathBuf) {
        if !self.blame_cache.contains_key(&path) {
            let target = path.to_string_lossy().into_owned();
            self.blame_cache
                .insert(path.clone(), crate::git::blame(&self.root, &target));
        }
        self.focus_or_open(EditorItem::Blame(path));
    }

    /// Run a staging action, then refresh the view + record feedback.
    fn git_action(&mut self, result: Result<(), String>, ok_msg: &str) {
        match result {
            Ok(()) => self.git_action_msg = Some((true, ok_msg.to_string())),
            Err(e) => self.git_action_msg = Some((false, e)),
        }
        self.refresh_git_view();
        self.git_refresh_at = Instant::now(); // refresh tree markers / Changes panel too
    }

    /// Drain this workspace's event stream into state (called by the supervisor).
    pub fn pump(&mut self) {
        let events: Vec<UiEvent> = self.rx.try_iter().collect();
        for event in events {
            self.apply_event(event);
        }
        self.poll_status();
    }

    /// While a turn is running, periodically ask the sidecar for a live metrics
    /// snapshot (conversation stats + concurrent sub-agents) for the dashboard.
    fn poll_status(&mut self) {
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

    fn apply_event(&mut self, event: UiEvent) {
        self.last_activity = Instant::now();
        match event {
            UiEvent::Ready { agent, model, cp_version, cwd } => {
                self.ready = true;
                self.agent = agent;
                self.model = model;
                self.cp_version = cp_version;
                self.cwd = cwd;
                if self.status != InstanceStatus::Dead {
                    self.set_status(InstanceStatus::Idle);
                }
                self.status_line = format!("Ready · {} · {}", self.agent, self.model);
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
                self.set_status(InstanceStatus::Idle);
                self.transcript.push(Entry::Agent(output));
            }
            UiEvent::CommandDone { handled, .. } => {
                self.running = false;
                self.turn_started = None;
                self.current_tool = None;
                self.sub_agents.clear();
                self.paused = false;
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
                self.set_status(InstanceStatus::Idle);
                self.transcript.push(Entry::Error(message));
            }
            UiEvent::Log(line) => self.logs.push(line),
            UiEvent::Status { stats, token_rate, sub_agents } => {
                self.run_stats = stats;
                self.token_rate = token_rate;
                self.sub_agents = sub_agents;
            }
            UiEvent::Paused(paused) => self.paused = paused,
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
    }

    fn set_status(&mut self, status: InstanceStatus) {
        if self.status != InstanceStatus::Dead {
            self.status = status;
        }
    }

    fn on_message(&mut self, msg: BackendMessage) {
        if msg.kind == "SpinnerControl" {
            return;
        }
        if msg.kind == "DiffMessage" {
            if let Some(record) = parse_diff(&msg) {
                // The AI just wrote this file — refresh an open editor buffer so
                // it shows the new content (unless the user has unsaved edits).
                let abs = self.abs_path(&record.path);
                if let Some(buf) = self.open_files.get_mut(&abs) {
                    if !buf.dirty {
                        if let Ok(content) = std::fs::read_to_string(&abs) {
                            buf.content = content;
                            buf.load_error = None;
                        }
                    }
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

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.ready || self.running {
            return;
        }
        if let Some(backend) = &self.backend {
            self.input.clear();
            self.transcript.push(Entry::User(text.clone()));
            if text.starts_with('/') {
                backend.send_command(&text);
            } else {
                backend.send_prompt(&text);
            }
            self.tool_calls = 0;
            self.current_tool = None;
            self.sub_agents.clear();
            self.paused = false;
            self.running = true;
            self.turn_started = Some(Instant::now());
            self.status_req_at = Instant::now();
            self.set_status(InstanceStatus::Running);
        }
    }

    /// Inject a steering message into the running turn (now = mid-turn, queue =
    /// after this turn). Clears the input box.
    fn steer(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.running {
            return;
        }
        let mode = if self.steer_queue_mode { "queue" } else { "now" };
        if let Some(backend) = &self.backend {
            backend.steer(&text, mode);
            self.input.clear();
            let tag = if self.steer_queue_mode { "📨 steer (queued)" } else { "🎯 steer" };
            self.transcript.push(Entry::User(format!("{tag}: {text}")));
        }
    }

    /// Toggle pause/resume of the running turn (optimistic; confirmed by event).
    fn toggle_pause(&mut self) {
        if !self.running {
            return;
        }
        if let Some(backend) = &self.backend {
            if self.paused {
                backend.resume();
            } else {
                backend.pause();
            }
            self.paused = !self.paused;
        }
    }

    fn answer_pending(&mut self) {
        let Some(pending) = self.pending.take() else { return };
        let Some(backend) = &self.backend else { return };
        match &pending.kind {
            PendingKind::Input { .. } => {
                backend.respond_input(&pending.prompt_id, &pending.text);
                self.transcript.push(Entry::User(format!("↳ {}", pending.text)));
            }
            PendingKind::Confirm { options, .. } => {
                let choice = options.get(pending.selection).cloned().unwrap_or_default();
                let confirmed = pending.selection == 0;
                backend.respond_confirmation(&pending.prompt_id, confirmed, None);
                self.transcript.push(Entry::User(format!("↳ {choice}")));
            }
            PendingKind::Select { options, .. } => {
                let value = options.get(pending.selection).cloned().unwrap_or_default();
                backend.respond_selection(&pending.prompt_id, pending.selection as i64, &value);
                self.transcript.push(Entry::User(format!("↳ {value}")));
            }
        }
        // Answering resumes the run.
        self.set_status(InstanceStatus::Running);
    }
}

// ===========================================================================
// Rendering — the chat tab for this workspace.
// ===========================================================================
impl Workspace {
    pub fn render_chat(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;
        self.poll_git(ui.ctx());

        let mut open_git = false;
        egui::Panel::top(egui::Id::new(("ws-top", id))).show_inside(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut self.show_tree, "🗂 Tree")
                    .on_hover_text("Toggle the file tree");
                if self.git_repo
                    && ui.button("🌿 Git").on_hover_text("Source control").clicked()
                {
                    open_git = true;
                }
                self.render_agent_picker(ui);
                self.render_model_picker(ui);
                ui.separator();
                ui.label(egui::RichText::new(&self.status_line).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_logs, "logs");
                    if self.running {
                        if self.paused {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 190, 110),
                                "⏸ paused",
                            );
                        } else {
                            ui.spinner();
                        }
                    }
                });
            });
        });

        if open_git {
            self.show_git();
        }

        // File tree sidebar (toggleable) — explorer (top) + Changes (bottom).
        if self.show_tree {
            let markers = self.tree_markers();
            // Snapshot the change list so the closures don't borrow self.
            let git_repo = self.git_repo;
            let git_list: Vec<(String, char)> = if git_repo {
                self.git_changes.iter().map(|c| (c.path.clone(), c.marker)).collect()
            } else {
                Vec::new()
            };
            let diff_list = if git_repo { Vec::new() } else { self.diff_changed_files() };
            let count = if git_repo { git_list.len() } else { diff_list.len() };

            let mut open_file: Option<PathBuf> = None;
            let mut click_diff: Option<usize> = None;
            let mut click_git: Option<(String, char)> = None;
            let mut do_refresh = false;

            egui::SidePanel::left(egui::Id::new(("ws-tree", id)))
                .resizable(true)
                .default_width(240.0)
                .show_inside(ui, |ui| {
                    // Source-control style Changes panel, pinned to the bottom.
                    egui::Panel::bottom(egui::Id::new(("ws-changes", id)))
                        .resizable(true)
                        .default_size(160.0)
                        .show_inside(ui, |ui| {
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("Changes ({count})")).strong());
                                if git_repo {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                                                do_refresh = true;
                                            }
                                        },
                                    );
                                }
                            });
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .id_salt(("changes-scroll", id))
                                .show(ui, |ui| {
                                    if count == 0 {
                                        ui.weak(if git_repo {
                                            "Working tree clean."
                                        } else {
                                            "No changes yet."
                                        });
                                    }
                                    if git_repo {
                                        for (path, marker) in &git_list {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    marker_color(*marker),
                                                    marker.to_string(),
                                                );
                                                if ui
                                                    .selectable_label(false, file_name(path))
                                                    .on_hover_text(path)
                                                    .clicked()
                                                {
                                                    click_git = Some((path.clone(), *marker));
                                                }
                                            });
                                        }
                                    } else {
                                        for (idx, path, marker) in &diff_list {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    marker_color(*marker),
                                                    marker.to_string(),
                                                );
                                                if ui
                                                    .selectable_label(false, file_name(path))
                                                    .on_hover_text(path)
                                                    .clicked()
                                                {
                                                    click_diff = Some(*idx);
                                                }
                                            });
                                        }
                                    }
                                });
                        });

                    // File tree fills the remaining (top) space.
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("🗂 {}", self.name)).strong());
                    ui.separator();
                    let mut clicked: Option<PathBuf> = None;
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt(("tree-scroll", id))
                        .show(ui, |ui| {
                            render_dir(ui, &self.root, &markers, &mut clicked);
                        });
                    if let Some(path) = clicked {
                        open_file = Some(path);
                    }
                });

            if do_refresh {
                self.git_refresh_at = Instant::now();
            }
            if let Some(path) = open_file {
                self.open_editor_file(path);
            }
            if let Some(i) = click_diff {
                self.load_diff_index(i);
            }
            if let Some((path, marker)) = click_git {
                self.load_git_diff(&path, marker);
            }
        }

        // Layout: with files/changes open, the editor fills the top and the
        // chat is pushed into a resizable bottom panel (IDE style). With nothing
        // open, the chat fills the whole area.
        if self.editor_open.is_empty() {
            self.render_chat_body(ui);
        } else {
            egui::Panel::bottom(egui::Id::new(("ws-chat", id)))
                .resizable(true)
                .default_size(280.0)
                .show_inside(ui, |ui| {
                    self.render_chat_body(ui);
                });
            self.render_editor_area(ui);
        }

        // Interactive question modal floats above everything for this workspace.
        if self.pending_ask.is_some() {
            self.render_ask_modal(ui.ctx());
        }
    }

    /// The chat region: transcript (scrolling) with the composer pinned to the
    /// bottom and the optional logs panel above it.
    fn render_chat_body(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;

        egui::Panel::bottom(egui::Id::new(("ws-composer", id))).show_inside(ui, |ui| {
            ui.add_space(4.0);
            if self.pending.is_some() {
                self.render_pending(ui);
            } else {
                self.render_composer(ui);
            }
            ui.add_space(4.0);
        });

        if self.show_logs {
            egui::Panel::bottom(egui::Id::new(("ws-logs", id)))
                .resizable(true)
                .default_size(120.0)
                .show_inside(ui, |ui| {
                    ui.label(egui::RichText::new("sidecar logs").weak());
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .id_salt(("ws-logs-scroll", id))
                        .show(ui, |ui| {
                            for line in &self.logs {
                                ui.label(egui::RichText::new(line).monospace().small());
                            }
                        });
                });
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .id_salt(("ws-transcript", id))
            .show(ui, |ui| {
                if self.transcript.is_empty() {
                    ui.weak("Ask Code Puppy to build, edit, or explain code.");
                }
                let cache = &mut self.md_cache;
                for entry in &self.transcript {
                    render_entry(ui, entry, cache);
                }
            });
    }

    /// The editor area: a tab bar of open files / Changes, then the active one.
    fn render_editor_area(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;
        let mut switch_to: Option<usize> = None;
        let mut close: Option<usize> = None;

        egui::Panel::top(egui::Id::new(("ws-editortabs", id))).show_inside(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, item) in self.editor_open.iter().enumerate() {
                    let selected = i == self.editor_active;
                    let label = match item {
                        EditorItem::Changes => "📝 Changes".to_string(),
                        EditorItem::Git => "🌿 Git".to_string(),
                        EditorItem::Commit { short, .. } => format!("⎇ {short}"),
                        EditorItem::Blame(p) => format!("🔍 {}", file_name(&p.to_string_lossy())),
                        EditorItem::File(p) => {
                            let dirty = self.open_files.get(p).map(|b| b.dirty).unwrap_or(false);
                            let name = p
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| p.to_string_lossy().into_owned());
                            format!("{}{name}", if dirty { "● " } else { "" })
                        }
                    };
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if ui.selectable_label(selected, label).clicked() {
                            switch_to = Some(i);
                        }
                        if ui.small_button("✕").clicked() {
                            close = Some(i);
                        }
                        ui.separator();
                    });
                }
            });
        });

        if let Some(i) = switch_to {
            self.editor_active = i;
        }
        if let Some(i) = close {
            self.close_editor(i);
        }

        if let Some(item) = self.editor_open.get(self.editor_active).cloned() {
            match item {
                EditorItem::Changes => self.render_diffs(ui),
                EditorItem::File(p) => self.render_file(ui, &p),
                EditorItem::Git => self.render_git(ui),
                EditorItem::Commit { hash, .. } => self.render_commit(ui, &hash),
                EditorItem::Blame(p) => self.render_blame(ui, &p),
            }
        }
    }

    /// The Changes tab: the colored diff for the change selected in the sidebar.
    pub fn render_diffs(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;
        let Some(d) = &self.current_diff else {
            ui.centered_and_justified(|ui| {
                ui.weak("Pick a file in the Changes panel to see its diff.");
            });
            return;
        };
        let (icon, color) = op_style(&d.operation);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, format!("{icon} {}", d.operation));
            ui.label(egui::RichText::new(&d.path).monospace());
            ui.label(egui::RichText::new(format!("+{}  −{}", d.adds, d.dels)).weak().small());
        });
        ui.separator();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("diff-scroll", id))
            .show(ui, |ui| {
                render_diff_lines(ui, &d.lines);
            });
    }

    /// The Git (Source Control) page: branch, staging, commit, history.
    fn render_git(&mut self, ui: &mut egui::Ui) {
        if self.git_view.is_none() {
            self.refresh_git_view();
        }
        let id = self.id.0;

        // Snapshot the view so the UI closures don't borrow `self`.
        let (branch, upstream, ahead, behind) = {
            let v = self.git_view.as_ref().unwrap();
            (v.branch.clone(), v.upstream, v.ahead, v.behind)
        };
        let staged: Vec<(String, char)> = self
            .git_view
            .as_ref()
            .unwrap()
            .staged
            .iter()
            .map(|e| (e.path.clone(), e.marker()))
            .collect();
        let unstaged: Vec<(String, char)> = self
            .git_view
            .as_ref()
            .unwrap()
            .unstaged
            .iter()
            .map(|e| (e.path.clone(), e.marker()))
            .collect();
        let log = self.git_view.as_ref().unwrap().log.clone();
        let action_msg = self.git_action_msg.clone();
        let can_commit = !staged.is_empty() && !self.commit_msg.trim().is_empty();

        let mut do_refresh = false;
        let mut do_commit = false;
        let mut do_stage_all = false;
        let mut do_unstage_all = false;
        let mut stage_path: Option<String> = None;
        let mut unstage_path: Option<String> = None;
        let mut diff_click: Option<(String, char)> = None;
        let mut commit_click: Option<crate::git::Commit> = None;

        // Branch header.
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("⎇ {branch}")).strong());
            if upstream {
                if ahead > 0 {
                    ui.label(egui::RichText::new(format!("↑{ahead}")).small());
                }
                if behind > 0 {
                    ui.label(egui::RichText::new(format!("↓{behind}")).small());
                }
                if ahead == 0 && behind == 0 {
                    ui.label(egui::RichText::new("up to date").weak().small());
                }
            } else {
                ui.label(egui::RichText::new("no upstream").weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                    do_refresh = true;
                }
            });
        });
        if let Some((ok, msg)) = &action_msg {
            let color = if *ok {
                egui::Color32::from_rgb(120, 200, 140)
            } else {
                egui::Color32::from_rgb(230, 120, 120)
            };
            ui.colored_label(color, msg);
        }
        ui.separator();

        // Commit box.
        ui.add(
            egui::TextEdit::multiline(&mut self.commit_msg)
                .id_salt(("commit-msg", id))
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("Commit message…"),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_commit, egui::Button::new("✓ Commit"))
                .on_hover_text(if staged.is_empty() {
                    "Stage something first"
                } else {
                    "Commit staged changes"
                })
                .clicked()
            {
                do_commit = true;
            }
            ui.label(
                egui::RichText::new(format!("{} staged", staged.len()))
                    .weak()
                    .small(),
            );
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(("git-scroll", id))
            .show(ui, |ui| {
                // Staged.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Staged ({})", staged.len())).strong());
                    if !staged.is_empty()
                        && ui.small_button("Unstage all").clicked()
                    {
                        do_unstage_all = true;
                    }
                });
                if staged.is_empty() {
                    ui.weak("Nothing staged.");
                }
                for (path, marker) in &staged {
                    ui.horizontal(|ui| {
                        if ui.small_button("−").on_hover_text("Unstage").clicked() {
                            unstage_path = Some(path.clone());
                        }
                        ui.colored_label(marker_color(*marker), marker.to_string());
                        if ui
                            .selectable_label(false, file_name(path))
                            .on_hover_text(path)
                            .clicked()
                        {
                            diff_click = Some((path.clone(), *marker));
                        }
                    });
                }

                ui.add_space(6.0);

                // Unstaged / untracked.
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Changes ({})", unstaged.len())).strong());
                    if !unstaged.is_empty() && ui.small_button("Stage all").clicked() {
                        do_stage_all = true;
                    }
                });
                if unstaged.is_empty() {
                    ui.weak("No unstaged changes.");
                }
                for (path, marker) in &unstaged {
                    ui.horizontal(|ui| {
                        if ui.small_button("+").on_hover_text("Stage").clicked() {
                            stage_path = Some(path.clone());
                        }
                        ui.colored_label(marker_color(*marker), marker.to_string());
                        if ui
                            .selectable_label(false, file_name(path))
                            .on_hover_text(path)
                            .clicked()
                        {
                            diff_click = Some((path.clone(), *marker));
                        }
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.label(egui::RichText::new("History").strong());
                for c in &log {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&c.short)
                                .monospace()
                                .small()
                                .color(egui::Color32::from_rgb(180, 150, 220)),
                        );
                        if ui
                            .selectable_label(false, &c.subject)
                            .on_hover_text(format!("{} · {}", c.author, c.when))
                            .clicked()
                        {
                            commit_click = Some(c.clone());
                        }
                    });
                }
            });

        // Apply deferred actions.
        if do_refresh {
            self.git_action_msg = None;
            self.refresh_git_view();
        }
        if let Some(p) = stage_path {
            let r = crate::git::stage(&self.root, &p);
            self.git_action(r, &format!("Staged {}", file_name(&p)));
        }
        if let Some(p) = unstage_path {
            let r = crate::git::unstage(&self.root, &p);
            self.git_action(r, &format!("Unstaged {}", file_name(&p)));
        }
        if do_stage_all {
            let r = crate::git::stage_all(&self.root);
            self.git_action(r, "Staged all changes");
        }
        if do_unstage_all {
            let r = crate::git::unstage_all(&self.root);
            self.git_action(r, "Unstaged all");
        }
        if do_commit {
            let msg = self.commit_msg.clone();
            match crate::git::commit(&self.root, &msg) {
                Ok(summary) => {
                    self.commit_msg.clear();
                    let line = summary.lines().next().unwrap_or("committed").to_string();
                    self.git_action(Ok(()), &format!("Committed · {line}"));
                }
                Err(e) => self.git_action(Err(e), ""),
            }
        }
        if let Some((path, marker)) = diff_click {
            self.load_git_diff(&path, marker);
        }
        if let Some(c) = commit_click {
            self.open_commit(&c);
        }
    }

    /// A single commit's patch (opened from the Git history list).
    fn render_commit(&mut self, ui: &mut egui::Ui, hash: &str) {
        let id = self.id.0;
        let matches = self
            .commit_view
            .as_ref()
            .map(|(h, _)| h == hash)
            .unwrap_or(false);
        if !matches {
            // A different commit tab is active; re-fetch this one.
            let text = crate::git::show(&self.root, hash);
            let (lines, adds, dels) = parse_unified(&text);
            self.commit_view = Some((
                hash.to_string(),
                DiffRecord {
                    path: hash.chars().take(8).collect(),
                    operation: "modify".to_string(),
                    adds,
                    dels,
                    lines,
                },
            ));
        }
        let Some((_, d)) = &self.commit_view else { return };
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&d.path).strong());
            ui.label(
                egui::RichText::new(format!("+{}  −{}", d.adds, d.dels))
                    .weak()
                    .small(),
            );
        });
        ui.separator();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("commit-scroll", id, hash))
            .show(ui, |ui| {
                render_diff_lines(ui, &d.lines);
            });
    }

    /// Per-line blame for a file.
    fn render_blame(&mut self, ui: &mut egui::Ui, path: &Path) {
        let id = self.id.0;
        let mut do_refresh = false;
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(path.display().to_string()).monospace().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Re-blame").clicked() {
                    do_refresh = true;
                }
            });
        });
        ui.separator();

        if do_refresh {
            let target = path.to_string_lossy().into_owned();
            self.blame_cache
                .insert(path.to_path_buf(), crate::git::blame(&self.root, &target));
        }

        let Some(lines) = self.blame_cache.get(path) else {
            ui.weak("No blame data.");
            return;
        };
        if lines.is_empty() {
            ui.weak("No blame data (not tracked, or git unavailable).");
            return;
        }
        let hash_color = egui::Color32::from_rgb(150, 130, 190);
        let meta_color = egui::Color32::from_gray(140);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("blame-scroll", id))
            .show(ui, |ui| {
                for bl in lines {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(egui::RichText::new(&bl.short).monospace().small().color(hash_color));
                        ui.label(
                            egui::RichText::new(format!("{:<12} {}", truncate(&bl.author, 12), bl.date))
                                .monospace()
                                .small()
                                .color(meta_color),
                        );
                        ui.add(
                            egui::Label::new(egui::RichText::new(&bl.line).monospace())
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
                }
            });
    }

    /// An editable file tab.
    pub fn render_file(&mut self, ui: &mut egui::Ui, path: &Path) {
        let git_repo = self.git_repo;
        let Some(buf) = self.open_files.get_mut(path) else {
            ui.centered_and_justified(|ui| {
                ui.weak("file not open");
            });
            return;
        };

        let mut do_save = false;
        let mut do_blame = false;
        egui::Panel::top(egui::Id::new(("file-bar", path))).show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(path.display().to_string()).monospace().small());
                if buf.dirty {
                    ui.colored_label(egui::Color32::from_rgb(220, 190, 110), "● unsaved");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("💾 Save").clicked() {
                        do_save = true;
                    }
                    if git_repo
                        && ui.button("🔍 Blame").on_hover_text("git blame").clicked()
                    {
                        do_blame = true;
                    }
                });
            });
            ui.add_space(2.0);
        });

        if let Some(err) = &buf.load_error {
            ui.colored_label(
                egui::Color32::from_rgb(240, 130, 130),
                format!("Cannot open file: {err}"),
            );
            return;
        }
        if let Some(err) = &buf.save_error {
            ui.colored_label(egui::Color32::from_rgb(240, 130, 130), format!("Save failed: {err}"));
        }

        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
        let lang = language_for(path);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("file-scroll", path))
            .show(ui, |ui| {
                let mut layouter =
                    |lui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
                        let mut job = egui_extras::syntax_highlighting::highlight(
                            lui.ctx(),
                            lui.style(),
                            &theme,
                            text.as_str(),
                            lang,
                        );
                        job.wrap.max_width = f32::INFINITY; // no wrap; horizontal scroll
                        lui.fonts_mut(|f| f.layout_job(job))
                    };
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut buf.content)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(40)
                        .layouter(&mut layouter),
                );
                if resp.changed() {
                    buf.dirty = true;
                }
            });

        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            do_save = true;
        }
        if do_save {
            match std::fs::write(path, buf.content.as_bytes()) {
                Ok(()) => {
                    buf.dirty = false;
                    buf.save_error = None;
                }
                Err(e) => buf.save_error = Some(e.to_string()),
            }
        }

        if do_blame {
            self.show_blame(path.to_path_buf());
        }
    }

    fn render_ask_modal(&mut self, ctx: &egui::Context) {
        let title = format!("🐶 Code Puppy asks — {}", self.name);
        // 0 = nothing, 1 = submit, 2 = cancel.
        let mut action = 0u8;
        {
            let Some(ask) = self.pending_ask.as_mut() else { return };
            egui::Window::new(title)
                .id(egui::Id::new(("ask-modal", ask.id.as_str())))
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    for q in &mut ask.questions {
                        ui.label(egui::RichText::new(&q.header).strong());
                        ui.label(&q.question);
                        ui.add_space(2.0);
                        for i in 0..q.options.len() {
                            let opt_label = q.options[i].label.clone();
                            let opt_desc = q.options[i].description.clone();
                            if q.multi_select {
                                ui.checkbox(&mut q.selected[i], &opt_label)
                                    .on_hover_text(&opt_desc);
                            } else if ui
                                .radio(q.selected[i], &opt_label)
                                .on_hover_text(&opt_desc)
                                .clicked()
                            {
                                for s in &mut q.selected {
                                    *s = false;
                                }
                                q.selected[i] = true;
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Other:").weak());
                            ui.text_edit_singleline(&mut q.other);
                        });
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Submit").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
        }

        match action {
            1 => {
                let ask = self.pending_ask.take().unwrap();
                let answers: Vec<AskAnswer> = ask
                    .questions
                    .iter()
                    .map(|q| {
                        let mut selected: Vec<String> = q
                            .options
                            .iter()
                            .zip(&q.selected)
                            .filter(|(_, s)| **s)
                            .map(|(o, _)| o.label.clone())
                            .collect();
                        let other = q.other.trim();
                        let other_text = if other.is_empty() {
                            None
                        } else {
                            selected.push(other.to_string());
                            Some(other.to_string())
                        };
                        AskAnswer {
                            question_header: q.header.clone(),
                            selected_options: selected,
                            other_text,
                        }
                    })
                    .collect();
                if let Some(backend) = &self.backend {
                    backend.ask_response(&ask.id, &answers);
                }
                let summary: Vec<String> = ask
                    .questions
                    .iter()
                    .map(|q| {
                        let picks: Vec<&str> = q
                            .options
                            .iter()
                            .zip(&q.selected)
                            .filter(|(_, s)| **s)
                            .map(|(o, _)| o.label.as_str())
                            .collect();
                        format!("{}: {}", q.header, picks.join(", "))
                    })
                    .collect();
                self.transcript
                    .push(Entry::User(format!("↳ {}", summary.join(" · "))));
                self.set_status(InstanceStatus::Running);
            }
            2 => {
                let ask = self.pending_ask.take().unwrap();
                if let Some(backend) = &self.backend {
                    backend.ask_cancel(&ask.id);
                }
                self.transcript
                    .push(Entry::Note("cancelled question".to_string()));
                self.set_status(InstanceStatus::Running);
            }
            _ => {}
        }
    }

    fn render_agent_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen: Option<String> = None;
        let current = if self.agent.is_empty() { "agent" } else { &self.agent };
        egui::ComboBox::from_id_salt(("agent-combo", self.id.0))
            .selected_text(format!("🐶 {current}"))
            .show_ui(ui, |ui| {
                for a in &self.agents {
                    let label = if a.display_name.is_empty() {
                        a.name.clone()
                    } else {
                        a.display_name.clone()
                    };
                    let resp = ui.selectable_label(a.current, label).on_hover_text(&a.description);
                    if resp.clicked() && !a.current {
                        chosen = Some(a.name.clone());
                    }
                }
            });
        if let Some(name) = chosen {
            if let Some(backend) = &self.backend {
                backend.set_agent(&name);
            }
        }
    }

    fn render_model_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen: Option<String> = None;
        let current = if self.model.is_empty() { "model" } else { &self.model };
        egui::ComboBox::from_id_salt(("model-combo", self.id.0))
            .selected_text(current.to_string())
            .show_ui(ui, |ui| {
                for m in &self.models {
                    let resp = ui
                        .selectable_label(m.current, &m.name)
                        .on_hover_text(&m.description);
                    if resp.clicked() && !m.current {
                        chosen = Some(m.name.clone());
                    }
                }
            });
        if let Some(name) = chosen {
            if let Some(backend) = &self.backend {
                backend.set_model(&name);
            }
        }
    }

    fn render_composer(&mut self, ui: &mut egui::Ui) {
        let mut apply = false;
        if self.comp_visible && !self.completions.is_empty() {
            let len = self.completions.len();
            let m = egui::Modifiers::NONE;
            if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowDown)) {
                self.comp_selected = (self.comp_selected + 1) % len;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowUp)) {
                self.comp_selected = (self.comp_selected + len - 1) % len;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Escape)) {
                self.comp_visible = false;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Tab)) {
                apply = true;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Enter)) {
                apply = true;
            }
        }

        if self.comp_visible {
            self.render_completion_popup(ui);
        }

        let mut stop = false;
        let mut do_submit = false;
        let mut do_steer = false;
        let mut do_pause = false;
        ui.horizontal(|ui| {
            // Commands menu to the left of the input box.
            self.render_commands_menu(ui);

            let running = self.running;
            // While a turn runs, the input box steers; otherwise it sends.
            let input_enabled = self.ready;
            let hint = if !self.ready {
                "Waiting for Code Puppy to start…"
            } else if running {
                "Steer Code Puppy mid-turn… (Enter to steer)"
            } else {
                "Message Code Puppy…  (/ for commands, @ for files)"
            };
            // Reserve room on the right for the action buttons.
            let reserve = if running { 250.0 } else { 70.0 };
            let field = ui.add_enabled(
                input_enabled,
                egui::TextEdit::singleline(&mut self.input)
                    .desired_width((ui.available_width() - reserve).max(60.0))
                    .hint_text(hint),
            );
            if self.request_input_focus {
                field.request_focus();
                self.request_input_focus = false;
            }
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            if running {
                // Steer delivery mode toggle (now = interrupt, queue = after turn).
                let mode_label = if self.steer_queue_mode { "📨 queue" } else { "🎯 now" };
                if ui
                    .selectable_label(false, mode_label)
                    .on_hover_text(
                        "Steer delivery — now: interrupt mid-turn · queue: after this turn",
                    )
                    .clicked()
                {
                    self.steer_queue_mode = !self.steer_queue_mode;
                }
                let steer = ui
                    .add_enabled(input_enabled, egui::Button::new("Steer"))
                    .on_hover_text("Send this as a steering message")
                    .clicked();
                let pause_label = if self.paused { "▶ Resume" } else { "⏸ Pause" };
                if ui
                    .button(pause_label)
                    .on_hover_text("Pause/resume the turn at the next safe point")
                    .clicked()
                {
                    do_pause = true;
                }
                if ui
                    .button("⏹ Stop")
                    .on_hover_text("Cancel the running turn")
                    .clicked()
                {
                    stop = true;
                }
                if !apply && (enter || steer) {
                    do_steer = true;
                }
            } else {
                let send = ui
                    .add_enabled(input_enabled, egui::Button::new("Send"))
                    .clicked();
                if !apply && (enter || send) {
                    do_submit = true;
                }
            }
        });

        if do_submit {
            self.submit();
            self.request_input_focus = true;
        }
        if do_steer {
            self.steer();
            self.request_input_focus = true;
        }
        if do_pause {
            self.toggle_pause();
        }
        if stop {
            if let Some(backend) = &self.backend {
                backend.cancel();
            }
            self.status_line = "Cancelling…".to_string();
        }
        if apply {
            self.apply_completion();
            self.request_input_focus = true;
        }

        self.maybe_request_completion();
    }

    fn apply_completion(&mut self) {
        let Some(item) = self.completions.get(self.comp_selected).cloned() else {
            return;
        };
        let remove = (-item.start_position).max(0) as usize;
        let char_len = self.input.chars().count();
        let keep = char_len.saturating_sub(remove);
        let prefix: String = self.input.chars().take(keep).collect();
        self.input = format!("{prefix}{}", item.text);
        self.comp_visible = false;
        self.last_query = self.input.clone();
    }

    fn render_completion_popup(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<usize> = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, c) in self.completions.iter().enumerate() {
                        let selected = i == self.comp_selected;
                        let resp = ui
                            .horizontal(|ui| {
                                let lab = ui.selectable_label(
                                    selected,
                                    egui::RichText::new(&c.display).monospace(),
                                );
                                if !c.meta.is_empty() {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(egui::RichText::new(&c.meta).weak().small());
                                        },
                                    );
                                }
                                lab
                            })
                            .inner;
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if selected {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                });
        });
        if let Some(i) = clicked {
            self.comp_selected = i;
            self.apply_completion();
            self.request_input_focus = true;
        }
    }

    fn maybe_request_completion(&mut self) {
        if !self.ready || self.running {
            self.comp_visible = false;
            return;
        }
        if self.input == self.last_query {
            return;
        }
        self.last_query = self.input.clone();
        let completable = self.input.starts_with('/') || self.input.contains('@');
        if !completable {
            self.comp_visible = false;
            self.completions.clear();
            return;
        }
        if let Some(backend) = &self.backend {
            self.comp_request_id =
                backend.request_completion(&self.input, self.input.chars().count());
        }
    }

    /// Commands menu with "smart" behavior: arg-less, non-interactive commands
    /// run on click; commands that take arguments (or open a terminal-only
    /// picker) are dropped into the composer for you to complete.
    fn render_commands_menu(&mut self, ui: &mut egui::Ui) {
        enum Pick {
            Run(String),
            Insert(String),
        }
        let mut pick: Option<Pick> = None;
        let enabled = self.ready && !self.running && !self.commands.is_empty();
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Commands ▾", |ui| {
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    let mut last_cat = "";
                    for c in &self.commands {
                        if c.category != last_cat {
                            if !last_cat.is_empty() {
                                ui.separator();
                            }
                            ui.label(egui::RichText::new(c.category.to_uppercase()).small().weak());
                            last_cat = &c.category;
                        }
                        let hover = format!("{}\n{}", c.usage, c.description);
                        if ui.button(format!("/{}", c.name)).on_hover_text(hover).clicked() {
                            pick = Some(if command_needs_input(c) {
                                Pick::Insert(c.name.clone())
                            } else {
                                Pick::Run(c.name.clone())
                            });
                            ui.close();
                        }
                    }
                });
            });
        });
        match pick {
            Some(Pick::Insert(name)) => {
                self.input = format!("/{name} ");
                self.request_input_focus = true;
            }
            Some(Pick::Run(name)) => {
                if let Some(backend) = &self.backend {
                    let cmd = format!("/{name}");
                    self.transcript.push(Entry::User(cmd.clone()));
                    backend.send_command(&cmd);
                    self.running = true;
                    self.turn_started = Some(Instant::now());
                    self.status_req_at = Instant::now();
                    self.set_status(InstanceStatus::Running);
                }
            }
            None => {}
        }
    }

    fn render_pending(&mut self, ui: &mut egui::Ui) {
        let mut submit = false;
        if let Some(pending) = &mut self.pending {
            match &pending.kind {
                PendingKind::Input { prompt, password } => {
                    ui.label(egui::RichText::new(prompt).strong());
                    ui.horizontal(|ui| {
                        let edit = egui::TextEdit::singleline(&mut pending.text)
                            .desired_width(ui.available_width() - 80.0)
                            .password(*password);
                        let field = ui.add(edit);
                        let enter =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Reply").clicked() || enter {
                            submit = true;
                        }
                    });
                }
                PendingKind::Confirm { title, description, options } => {
                    ui.label(egui::RichText::new(title).strong());
                    if !description.is_empty() {
                        ui.label(description);
                    }
                    ui.horizontal(|ui| {
                        for (i, opt) in options.iter().enumerate() {
                            if ui.button(opt).clicked() {
                                pending.selection = i;
                                submit = true;
                            }
                        }
                    });
                }
                PendingKind::Select { prompt, options } => {
                    ui.label(egui::RichText::new(prompt).strong());
                    for (i, opt) in options.iter().enumerate() {
                        if ui.selectable_label(pending.selection == i, opt).clicked() {
                            pending.selection = i;
                        }
                    }
                    if ui.button("Select").clicked() {
                        submit = true;
                    }
                }
            }
        }
        if submit {
            self.answer_pending();
        }
    }
}

/// A command "needs input" if its usage shows a placeholder (`<...>` / `[...]`)
/// or it's a known interactive picker that can't run headless.
fn command_needs_input(c: &CommandInfo) -> bool {
    const INTERACTIVE: &[&str] = &[
        "agent", "model", "mcp", "add_model", "diff", "colors", "model_settings",
        "set", "tutorial", "judges",
    ];
    c.usage.contains('<')
        || c.usage.contains('[')
        || INTERACTIVE.contains(&c.name.as_str())
}

fn parse_diff(msg: &BackendMessage) -> Option<DiffRecord> {
    let p = &msg.payload;
    let path = p.get("path")?.as_str()?.to_string();
    let operation = p
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("modify")
        .to_string();
    let mut lines = Vec::new();
    let (mut adds, mut dels) = (0usize, 0usize);
    if let Some(arr) = p.get("diff_lines").and_then(Value::as_array) {
        for l in arr {
            let kind = l.get("type").and_then(Value::as_str).unwrap_or("context").to_string();
            let content = l.get("content").and_then(Value::as_str).unwrap_or("").to_string();
            match kind.as_str() {
                "add" => adds += 1,
                "remove" => dels += 1,
                _ => {}
            }
            lines.push(DiffLine { kind, content });
        }
    }
    Some(DiffRecord { path, operation, adds, dels, lines })
}

/// Recursively render a directory as a lazy collapsible tree. Only expanded
/// folders are read (the collapsing body runs only when open).
fn render_dir(
    ui: &mut egui::Ui,
    dir: &Path,
    markers: &HashMap<PathBuf, char>,
    clicked: &mut Option<PathBuf>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(bool, PathBuf, String)> = read
        .filter_map(|e| e.ok())
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (is_dir, path, name)
        })
        .filter(|(is_dir, _, name)| {
            !name.is_empty() && !(*is_dir && TREE_IGNORE.contains(&name.as_str()))
        })
        .collect();
    // Directories first, then case-insensitive alphabetical.
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });

    for (is_dir, path, name) in entries {
        if is_dir {
            egui::CollapsingHeader::new(format!("📁 {name}"))
                .id_salt(&path)
                .show(ui, |ui| render_dir(ui, &path, markers, clicked));
        } else {
            let marker = markers.get(&path).copied();
            let resp = ui
                .horizontal(|ui| {
                    let r = ui.selectable_label(false, format!("📄 {name}"));
                    if let Some(m) = marker {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| ui.colored_label(marker_color(m), m.to_string()),
                        );
                    }
                    r
                })
                .inner;
            if resp.clicked() {
                *clicked = Some(path);
            }
        }
    }
}

/// Map a file extension to a syntect language token for highlighting.
fn language_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rs",
        "py" | "pyw" => "py",
        "toml" => "toml",
        "json" => "json",
        "md" | "markdown" => "md",
        "js" | "mjs" | "cjs" => "js",
        "ts" | "tsx" => "ts",
        "html" | "htm" => "html",
        "css" => "css",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
        "go" => "go",
        "java" => "java",
        "yaml" | "yml" => "yaml",
        "xml" => "xml",
        "sql" => "sql",
        "rb" => "rb",
        "php" => "php",
        "lua" => "lua",
        _ => "txt",
    }
}

/// A short, friendly tool name derived from a tool-output message kind.
fn tool_label(kind: &str) -> String {
    match kind {
        "ShellStartMessage" | "ShellOutputMessage" | "ShellLineMessage" => "shell",
        "FileListingMessage" => "list_files",
        "FileContentMessage" => "read_file",
        "GrepResultMessage" => "grep",
        "DiffMessage" => "edit",
        "SkillListMessage" | "SkillActivateMessage" => "skills",
        "SubAgentInvocationMessage" | "SubAgentResponseMessage" | "SubAgentStatusMessage" => {
            "sub-agent"
        }
        "UniversalConstructorMessage" => "uc",
        other => other,
    }
    .to_string()
}

fn op_marker(operation: &str) -> char {
    match operation {
        "create" => 'A',
        "delete" => 'D',
        _ => 'M',
    }
}

fn marker_color(marker: char) -> egui::Color32 {
    match marker {
        'A' | '?' => egui::Color32::from_rgb(120, 200, 130),
        'D' => egui::Color32::from_rgb(230, 120, 120),
        _ => egui::Color32::from_rgb(220, 180, 100),
    }
}

/// Parse a unified diff (git output) into renderable lines + add/del counts.
fn parse_unified(text: &str) -> (Vec<DiffLine>, usize, usize) {
    let mut lines = Vec::new();
    let (mut adds, mut dels) = (0usize, 0usize);
    for l in text.lines() {
        if l.starts_with("diff ")
            || l.starts_with("index ")
            || l.starts_with("--- ")
            || l.starts_with("+++ ")
            || l.starts_with("new file")
            || l.starts_with("deleted file")
            || l.starts_with("old mode")
            || l.starts_with("new mode")
            || l.starts_with("similarity")
            || l.starts_with("rename ")
        {
            continue;
        }
        if l.starts_with("@@") {
            lines.push(DiffLine { kind: "context".into(), content: l.to_string() });
        } else if let Some(rest) = l.strip_prefix('+') {
            adds += 1;
            lines.push(DiffLine { kind: "add".into(), content: rest.to_string() });
        } else if let Some(rest) = l.strip_prefix('-') {
            dels += 1;
            lines.push(DiffLine { kind: "remove".into(), content: rest.to_string() });
        } else {
            let rest = l.strip_prefix(' ').unwrap_or(l);
            lines.push(DiffLine { kind: "context".into(), content: rest.to_string() });
        }
    }
    (lines, adds, dels)
}

fn op_style(operation: &str) -> (&'static str, egui::Color32) {
    match operation {
        "create" => ("＋", egui::Color32::from_rgb(150, 220, 150)),
        "delete" => ("🗑", egui::Color32::from_rgb(240, 130, 130)),
        _ => ("✎", egui::Color32::from_rgb(220, 190, 110)),
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Truncate to `max` chars (for fixed-width blame columns).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn render_diff_lines(ui: &mut egui::Ui, lines: &[DiffLine]) {
    let add = egui::Color32::from_rgb(120, 220, 140);
    let rem = egui::Color32::from_rgb(240, 140, 140);
    let ctx = egui::Color32::from_gray(150);
    for l in lines {
        let (color, sign) = match l.kind.as_str() {
            "add" => (add, "+"),
            "remove" => (rem, "−"),
            _ => (ctx, " "),
        };
        let text = egui::RichText::new(format!("{sign} {}", l.content)).monospace();
        ui.add(
            egui::Label::new(text.color(color))
                .selectable(true)
                .wrap_mode(egui::TextWrapMode::Extend),
        );
    }
}

fn parse_pending(msg: &BackendMessage) -> Option<Pending> {
    let p = &msg.payload;
    let prompt_id = p.get("prompt_id")?.as_str()?.to_string();
    let kind = match msg.kind.as_str() {
        "UserInputRequest" => PendingKind::Input {
            prompt: str_field(p, "prompt_text").unwrap_or_else(|| "Input:".into()),
            password: str_field(p, "input_type").as_deref() == Some("password"),
        },
        "ConfirmationRequest" => PendingKind::Confirm {
            title: str_field(p, "title").unwrap_or_else(|| "Confirm".into()),
            description: str_field(p, "description").unwrap_or_default(),
            options: str_vec(p, "options").unwrap_or_else(|| vec!["Yes".into(), "No".into()]),
        },
        "SelectionRequest" => PendingKind::Select {
            prompt: str_field(p, "prompt_text").unwrap_or_else(|| "Select:".into()),
            options: str_vec(p, "options").unwrap_or_default(),
        },
        _ => return None,
    };
    Some(Pending { prompt_id, kind, text: String::new(), selection: 0 })
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn str_vec(v: &Value, key: &str) -> Option<Vec<String>> {
    v.get(key)?
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

const AGENT_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 220, 150);

fn render_markdown(ui: &mut egui::Ui, cache: &mut CommonMarkCache, text: &str) {
    CommonMarkViewer::new().show(ui, cache, text);
}

fn render_entry(ui: &mut egui::Ui, entry: &Entry, cache: &mut CommonMarkCache) {
    match entry {
        Entry::User(text) => labelled(ui, "you", egui::Color32::from_rgb(120, 170, 255), text),
        Entry::Agent(text) => {
            ui.colored_label(AGENT_COLOR, "Code Puppy:");
            render_markdown(ui, cache, text);
            ui.add_space(6.0);
        }
        Entry::Note(text) => {
            ui.label(egui::RichText::new(text).weak().italics());
            ui.add_space(4.0);
        }
        Entry::Error(text) => {
            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), format!("⚠ {text}"));
            ui.add_space(4.0);
        }
        Entry::Message(msg) => render_message(ui, msg, cache),
    }
}

fn render_message(ui: &mut egui::Ui, msg: &BackendMessage, cache: &mut CommonMarkCache) {
    // Agent prose is markdown — render it formatted.
    if msg.category == "agent" {
        ui.label(egui::RichText::new("Code Puppy").color(AGENT_COLOR).small());
        render_markdown(ui, cache, &msg.text);
        ui.add_space(2.0);
        return;
    }
    let color = match msg.category.as_str() {
        "tool_output" => egui::Color32::from_rgb(200, 180, 120),
        "user_interaction" => egui::Color32::from_rgb(220, 160, 220),
        "divider" => egui::Color32::DARK_GRAY,
        _ => egui::Color32::GRAY,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("[{}]", msg.kind)).color(color).small());
        ui.label(&msg.text);
    });
    ui.add_space(2.0);
}

fn labelled(ui: &mut egui::Ui, who: &str, color: egui::Color32, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color, format!("{who}:"));
        ui.label(text);
    });
    ui.add_space(6.0);
}
