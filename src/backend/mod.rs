//! Code Puppy backend: process management + JSON protocol bridge.
//!
//! We don't reimplement Code Puppy — we run the real Python package in a
//! sidecar process ([`sidecar.py`], embedded at build time) and talk to it over
//! line-delimited JSON on stdio. This module:
//!
//!   1. Resolves how to launch the sidecar (detect an existing `code_puppy`
//!      install, else auto-provision one with `uv`).
//!   2. Spawns it, wires reader threads for stdout (protocol) and stderr (logs).
//!   3. Exposes a handle to send user turns and input responses.
//!
//! The GUI consumes [`UiEvent`]s from a channel and never blocks on the agent.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::Value;

use crate::waker::UiWaker;

pub mod creds_push;
mod protocol;
pub mod remote;
pub mod ssh;
pub mod ssh_fallback;
mod types;

/// The sidecar source, embedded so the executable is self-contained.
const SIDECAR_PY: &str = include_str!("../../sidecar/sidecar.py");

pub use types::*;

/// Events flowing from the backend to the GUI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Sidecar initialized and the agent is ready (also re-sent on agent/model switch).
    Ready {
        agent: String,
        model: String,
        cp_version: String,
        cwd: String,
        autosave: String,
        puppy_name: String,
        owner_name: String,
    },
    /// A streamed message from the agent (assistant text, tool output, etc.).
    Message(BackendMessage),
    /// The catalog of available slash commands.
    Commands(Vec<CommandInfo>),
    /// The sidecar's working directory changed (`/cd` — workspaces follow).
    Cwd(String),
    /// The catalog of available agents.
    Agents { items: Vec<AgentInfo>, open: bool },
    /// The catalog of available models.
    Models { items: Vec<ModelInfo>, open: bool },
    /// Completion candidates for a `request_completion` (correlated by `id`).
    Completions { id: u64, items: Vec<CompletionItem> },
    /// An interactive question — answer via `ask_response` / `ask_cancel`.
    Ask {
        id: String,
        questions: Vec<AskQuestion>,
    },
    /// A user turn finished; `output` is the agent's final response.
    Result {
        #[allow(dead_code)] // correlation id; results apply to the active turn
        id: u64,
        output: String,
    },
    /// A slash command finished (`handled` = whether the dispatcher claimed it).
    CommandDone {
        #[allow(dead_code)]
        id: u64,
        handled: bool,
    },
    /// An error tied to a turn (`id`) or the bridge itself (`id = None`).
    Error {
        #[allow(dead_code)]
        id: Option<u64>,
        message: String,
    },
    /// Diagnostic log line (sidecar stderr or internal notices).
    Log(String),
    /// Live run metrics snapshot (answer to `request_status`).
    Status {
        stats: String,
        token_rate: f64,
        sub_agents: Vec<SubAgentInfo>,
        /// The running turn is held at the PauseController gate.
        paused: bool,
        /// Steering messages waiting to be drained (now + queue mode).
        queued: u64,
        /// The last user prompt sent to the agent (for the redesign's cards).
        last_prompt: String,
        /// Cumulative provider-reported tokens across all turns this session.
        total_tokens: u64,
        /// Context-window utilization 0–100 (the sidecar delegates to Code
        /// Puppy's own /context estimator); `None` when unknowable.
        ctx_pct: Option<f64>,
        /// Full /context breakdown for the popover; `None` when unknowable.
        ctx: Option<ContextBreakdown>,
        /// Cumulative $ cost — priced from the library's bundled models.dev
        /// snapshot; `None` means unknown (e.g. subscription models), not free.
        cost: Option<f64>,
        /// `cost` is an estimate (dated snapshot, cache discounts unmodeled).
        cost_estimated: bool,
    },
    /// The running turn was paused (`true`) or resumed (`false`).
    Paused(bool),
    /// The catalog of saved Code Puppy sessions (answer to `list_sessions`).
    /// `open` requests the GUI to pop the picker (from `/resume`).
    Sessions {
        items: Vec<SessionInfo>,
        current: String,
        open: bool,
    },
    /// A session was loaded; `entries` is the reconstructed transcript.
    SessionLoaded {
        name: String,
        messages: u64,
        entries: Vec<SessionEntry>,
    },
    /// A read-only preview of a session's conversation (not loaded into the agent).
    SessionPreview {
        name: String,
        #[allow(dead_code)]
        messages: u64,
        entries: Vec<SessionEntry>,
    },
    /// The catalog of registered MCP servers (answer to `list_mcp_servers`).
    McpServers(Vec<McpServerInfo>),
    /// The catalog of discovered skills (answer to `list_skills`).
    Skills(Vec<SkillInfo>),
    /// One skill's full SKILL.md content (answer to `get_skill`).
    SkillDetail(SkillDetail),
    /// The agent catalog + tool/MCP option lists (answer to
    /// `list_agent_configs`). The catalogs feed the visual builder.
    AgentConfigs {
        items: Vec<AgentConfigInfo>,
        available_tools: Vec<String>,
        available_mcp: Vec<String>,
    },
    /// One agent's full config (answer to `get_agent_config`).
    AgentConfigDetail(AgentConfigDetail),
    /// Kennel-wide totals (answer to `kennel_stats`).
    KennelStats(KennelStats),
    /// Every kennel wing + drawer count (answer to `kennel_list_wings`).
    KennelWings(Vec<KennelWing>),
    /// A page of remembered drawers (answer to `kennel_recent`/`kennel_search`).
    KennelDrawers(Vec<KennelDrawer>),
    /// The configured judge roster (answer to `list_judges`).
    Judges(Vec<JudgeInfo>),
    /// One judge's full config (answer to `get_judge`).
    JudgeDetail(JudgeDetail),
    /// The sidecar process exited.
    Exited { code: Option<i32> },
}

/// The wire form of sidecar -> GUI events (deserialized from each stdout line).
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Wire {
    Ready {
        #[serde(default)]
        agent: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        cp_version: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        autosave: String,
        #[serde(default)]
        puppy_name: String,
        #[serde(default)]
        owner_name: String,
    },
    Message {
        #[serde(default)]
        source: String,
        #[serde(default)]
        kind: String,
        #[serde(default)]
        category: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        payload: Value,
    },
    Commands {
        #[serde(default)]
        items: Vec<CommandInfo>,
    },
    /// The sidecar's working directory changed (`/cd`).
    Cwd {
        #[serde(default)]
        path: String,
    },
    Agents {
        #[serde(default)]
        items: Vec<AgentInfo>,
        /// Bare `/agent`: the sidecar asks the GUI to open its switcher.
        #[serde(default)]
        open: bool,
    },
    Models {
        #[serde(default)]
        items: Vec<ModelInfo>,
        /// Bare `/model`: the sidecar asks the GUI to open its switcher.
        #[serde(default)]
        open: bool,
    },
    Completions {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        items: Vec<CompletionItem>,
    },
    Ask {
        #[serde(default)]
        id: String,
        #[serde(default)]
        questions: Vec<AskQuestion>,
    },
    Result {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        output: String,
    },
    CommandDone {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        handled: bool,
    },
    Error {
        #[serde(default)]
        id: Option<u64>,
        #[serde(default)]
        message: String,
    },
    Log {
        #[serde(default)]
        text: String,
    },
    Status {
        #[serde(default)]
        stats: String,
        #[serde(default)]
        token_rate: f64,
        #[serde(default)]
        sub_agents: Vec<SubAgentInfo>,
        #[serde(default)]
        paused: bool,
        #[serde(default)]
        queued: u64,
        #[serde(default)]
        last_prompt: String,
        #[serde(default)]
        total_tokens: u64,
        #[serde(default)]
        ctx_pct: Option<f64>,
        #[serde(default)]
        ctx: Option<ContextBreakdown>,
        #[serde(default)]
        cost: Option<f64>,
        #[serde(default)]
        cost_estimated: bool,
    },
    Paused {
        #[serde(default)]
        paused: bool,
    },
    Sessions {
        #[serde(default)]
        items: Vec<SessionInfo>,
        #[serde(default)]
        current: String,
        #[serde(default)]
        open: bool,
    },
    SessionLoaded {
        #[serde(default)]
        name: String,
        #[serde(default)]
        messages: u64,
        #[serde(default)]
        entries: Vec<SessionEntry>,
    },
    SessionPreview {
        #[serde(default)]
        name: String,
        #[serde(default)]
        messages: u64,
        #[serde(default)]
        entries: Vec<SessionEntry>,
    },
    McpServers {
        #[serde(default)]
        items: Vec<McpServerInfo>,
    },
    Skills {
        #[serde(default)]
        items: Vec<SkillInfo>,
    },
    SkillDetail {
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        content: String,
    },
    AgentConfigs {
        #[serde(default)]
        items: Vec<AgentConfigInfo>,
        #[serde(default)]
        available_tools: Vec<String>,
        #[serde(default)]
        available_mcp: Vec<String>,
    },
    AgentConfig {
        #[serde(default)]
        name: String,
        #[serde(default)]
        display_name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        system_prompt: String,
        #[serde(default)]
        user_prompt: Option<String>,
        #[serde(default)]
        model: String,
        #[serde(default)]
        tools: Vec<String>,
        #[serde(default)]
        mcp_servers: Vec<String>,
        #[serde(default)]
        editable: bool,
        #[serde(default)]
        source: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        content: String,
    },
    KennelStats {
        #[serde(default)]
        drawers: u64,
        #[serde(default)]
        wings: u64,
        #[serde(default)]
        bytes: u64,
    },
    KennelWings {
        #[serde(default)]
        items: Vec<KennelWing>,
    },
    KennelDrawers {
        #[serde(default)]
        items: Vec<KennelDrawer>,
    },
    Judges {
        #[serde(default)]
        items: Vec<JudgeInfo>,
    },
    JudgeDetail {
        #[serde(default)]
        name: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        prompt: String,
        #[serde(default)]
        enabled: bool,
    },
}

impl From<Wire> for UiEvent {
    fn from(w: Wire) -> Self {
        match w {
            Wire::Ready {
                agent,
                model,
                cp_version,
                cwd,
                autosave,
                puppy_name,
                owner_name,
            } => UiEvent::Ready {
                agent,
                model,
                cp_version,
                cwd,
                autosave,
                puppy_name,
                owner_name,
            },
            Wire::Message {
                source,
                kind,
                category,
                text,
                payload,
            } => UiEvent::Message(BackendMessage {
                source,
                kind,
                category,
                text,
                payload,
            }),
            Wire::Commands { items } => UiEvent::Commands(items),
            Wire::Cwd { path } => UiEvent::Cwd(path),
            Wire::Agents { items, open } => UiEvent::Agents { items, open },
            Wire::Models { items, open } => UiEvent::Models { items, open },
            Wire::Completions { id, items } => UiEvent::Completions { id, items },
            Wire::Ask { id, questions } => UiEvent::Ask { id, questions },
            Wire::Result { id, output } => UiEvent::Result { id, output },
            Wire::CommandDone { id, handled } => UiEvent::CommandDone { id, handled },
            Wire::Error { id, message } => UiEvent::Error { id, message },
            Wire::Log { text } => UiEvent::Log(text),
            Wire::Status {
                stats,
                token_rate,
                sub_agents,
                paused,
                queued,
                last_prompt,
                total_tokens,
                ctx_pct,
                ctx,
                cost,
                cost_estimated,
            } => UiEvent::Status {
                stats,
                token_rate,
                sub_agents,
                paused,
                queued,
                last_prompt,
                total_tokens,
                ctx_pct,
                ctx,
                cost,
                cost_estimated,
            },
            Wire::Paused { paused } => UiEvent::Paused(paused),
            Wire::Sessions {
                items,
                current,
                open,
            } => UiEvent::Sessions {
                items,
                current,
                open,
            },
            Wire::SessionLoaded {
                name,
                messages,
                entries,
            } => UiEvent::SessionLoaded {
                name,
                messages,
                entries,
            },
            Wire::SessionPreview {
                name,
                messages,
                entries,
            } => UiEvent::SessionPreview {
                name,
                messages,
                entries,
            },
            Wire::McpServers { items } => UiEvent::McpServers(items),
            Wire::Skills { items } => UiEvent::Skills(items),
            Wire::SkillDetail {
                name,
                description,
                path,
                content,
            } => UiEvent::SkillDetail(SkillDetail {
                name,
                description,
                path,
                content,
            }),
            Wire::AgentConfigs {
                items,
                available_tools,
                available_mcp,
            } => UiEvent::AgentConfigs {
                items,
                available_tools,
                available_mcp,
            },
            Wire::AgentConfig {
                name,
                display_name,
                description,
                system_prompt,
                user_prompt,
                model,
                tools,
                mcp_servers,
                editable,
                source,
                path,
                content,
            } => UiEvent::AgentConfigDetail(AgentConfigDetail {
                name,
                display_name,
                description,
                system_prompt,
                user_prompt,
                model,
                tools,
                mcp_servers,
                editable,
                source,
                path,
                content,
            }),
            Wire::KennelStats {
                drawers,
                wings,
                bytes,
            } => UiEvent::KennelStats(KennelStats {
                drawers,
                wings,
                bytes,
            }),
            Wire::KennelWings { items } => UiEvent::KennelWings(items),
            Wire::KennelDrawers { items } => UiEvent::KennelDrawers(items),
            Wire::Judges { items } => UiEvent::Judges(items),
            Wire::JudgeDetail {
                name,
                model,
                prompt,
                enabled,
            } => UiEvent::JudgeDetail(JudgeDetail {
                name,
                model,
                prompt,
                enabled,
            }),
        }
    }
}

/// A freshly spawned remote sidecar: the handle, its event stream, and the
/// remote filesystem + git backends the workspace drives the tree/editor/git
/// through.
pub type RemoteHandle = (
    CodePuppy,
    Receiver<UiEvent>,
    Arc<dyn crate::workspace::fs::WorkspaceFs>,
    Arc<dyn crate::git::WorkspaceGit>,
);

/// Why a remote spawn failed -- structured so the UI can offer SSH-fallback
/// mode for "can't host" and ONLY for "can't host".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// Auth + shell are fine (provisioning succeeded) but the sidecar
    /// launcher is missing on the remote.
    CannotHost { launcher: String },
    /// Everything else: auth, network, ssh-level failures. Never triggers
    /// a fallback offer.
    Other(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::CannotHost { launcher } => write!(
                f,
                "the remote can't run Code Puppy (`{launcher}` not found)"
            ),
            RemoteError::Other(e) => f.write_str(e),
        }
    }
}

impl From<String> for RemoteError {
    fn from(e: String) -> Self {
        RemoteError::Other(e)
    }
}

impl From<&str> for RemoteError {
    fn from(e: &str) -> Self {
        RemoteError::Other(e.to_string())
    }
}

/// A running Code Puppy sidecar we can drive.
pub struct CodePuppy {
    child: Child,
    /// Shared so a [`remote::RemoteState`] can also send RPC ops down the pipe.
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: AtomicU64,
}

impl CodePuppy {
    /// Launch the sidecar. Returns the handle and the event stream, or an error
    /// string if the process couldn't even be started.
    ///
    /// `waker` is used to wake the GUI when events arrive.
    /// `cwd`, when set, becomes the sidecar's working directory — Code Puppy's
    /// `os.getcwd()` adopts it, scoping the whole process to that workspace.
    pub fn spawn(
        waker: Arc<dyn UiWaker>,
        cwd: Option<&std::path::Path>,
    ) -> Result<(Self, Receiver<UiEvent>), String> {
        let sidecar_path = write_sidecar().map_err(|e| format!("writing sidecar: {e}"))?;

        let mut command = resolve_launch(&sidecar_path)?;
        crate::proc::hide_console(&mut command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("spawning Code Puppy sidecar: {e}"))?;

        let stdin = child.stdin.take().ok_or("no stdin pipe")?;
        let stdout = child.stdout.take().ok_or("no stdout pipe")?;
        let stderr = child.stderr.take().ok_or("no stderr pipe")?;

        let (tx, rx) = mpsc::channel::<UiEvent>();

        // stdout -> protocol events (no RPC dispatch for a local sidecar)
        spawn_stdout_reader(stdout, tx.clone(), waker.clone(), None);
        // stderr -> log events
        spawn_stderr_reader(stderr, tx.clone(), waker);

        Ok((
            CodePuppy {
                child,
                stdin: Arc::new(Mutex::new(stdin)),
                next_id: AtomicU64::new(1),
            },
            rx,
        ))
    }

    /// Launch the sidecar on a remote host over the user's `ssh`. Provisions
    /// `sidecar.py` to the remote cache, then runs it; the returned handle and
    /// event stream are identical to [`spawn`](Self::spawn) -- the JSON protocol
    /// is transport-agnostic. `remote_cwd` is a path *on the remote*.
    ///
    /// Errors are structured: [`RemoteError::CannotHost`] fires only AFTER
    /// provisioning succeeded (auth + network proven good) when the launcher
    /// binary is missing on the remote -- the UI offers SSH-fallback mode for
    /// that case and ONLY that case (auth failures must never silently
    /// switch modes).
    pub fn spawn_remote(
        waker: Arc<dyn UiWaker>,
        target: &ssh::SshTarget,
        remote_cwd: Option<&str>,
    ) -> Result<RemoteHandle, RemoteError> {
        // 1. Provision: ship sidecar.py to the remote cache (bytes on stdin).
        let mut prov = target.provision_command();
        crate::proc::hide_console(&mut prov);
        prov.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut prov_child = prov
            .spawn()
            .map_err(|e| format!("starting ssh to provision sidecar: {e}"))?;
        let send_err = {
            let mut si = prov_child.stdin.take().ok_or("no stdin pipe (provision)")?;
            si.write_all(SIDECAR_PY.as_bytes()).err()
            // `si` drops here -> EOF, so the remote `cat` finishes.
        };
        let prov_out = prov_child
            .wait_with_output()
            .map_err(|e| format!("waiting on remote provisioning: {e}"))?;
        if !prov_out.status.success() || send_err.is_some() {
            // Prefer ssh's own stderr ("Could not resolve hostname", auth
            // refusals) over a raw broken-pipe write error — when ssh dies
            // early the pipe breaks FIRST, but the reason is on stderr.
            let err = String::from_utf8_lossy(&prov_out.stderr);
            let err = err.trim();
            return Err(RemoteError::Other(if !err.is_empty() {
                format!("remote provisioning failed: {err}")
            } else if let Some(e) = send_err {
                format!("sending sidecar to remote: {e}")
            } else {
                "remote provisioning failed (check the SSH target and auth)".to_string()
            }));
        }

        // 2. Preflight: can this host RUN the sidecar? Provisioning proved
        // auth + a POSIX shell; now `command -v <launcher argv0>`. Exit 1 =
        // launcher missing (CannotHost -> the UI can offer SSH-fallback);
        // exit 255 = ssh-level failure mid-flow (NOT a hosting verdict).
        let launcher = ssh::default_remote_launcher();
        let mut probe = target.launcher_probe_command(&launcher, remote_cwd);
        crate::proc::hide_console(&mut probe);
        probe
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let probe_status = probe
            .status()
            .map_err(|e| format!("probing the remote launcher: {e}"))?;
        if !probe_status.success() {
            let argv0 = launcher.split_whitespace().next().unwrap_or("uv");
            return Err(match probe_status.code() {
                // Exit 3: the requested path doesn't exist — a plain error
                // on ANY connect mode (previously this "connected" and the
                // sidecar died on `cd` right after adoption).
                Some(3) => RemoteError::Other(format!(
                    "remote path {} doesn't exist",
                    remote_cwd.unwrap_or("?")
                )),
                Some(255) => {
                    RemoteError::Other("ssh failed while probing the remote launcher".to_string())
                }
                _ => RemoteError::CannotHost {
                    launcher: argv0.to_string(),
                },
            });
        }

        // 3. Launch: run the remote sidecar; stdio carries the protocol.
        let mut command = target.launch_command(remote_cwd, &launcher);
        crate::proc::hide_console(&mut command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| format!("spawning remote sidecar over ssh: {e}"))?;

        let stdin = child.stdin.take().ok_or("no stdin pipe")?;
        let stdout = child.stdout.take().ok_or("no stdout pipe")?;
        let stderr = child.stderr.take().ok_or("no stderr pipe")?;

        // Share the stdin so the remote fs/git can send RPC ops down the pipe.
        let stdin = Arc::new(Mutex::new(stdin));
        let remote = remote::RemoteState::new(stdin.clone(), waker.clone());
        let remote_fs: Arc<dyn crate::workspace::fs::WorkspaceFs> =
            Arc::new(remote::RemoteFs::new(remote.clone()));
        let remote_git: Arc<dyn crate::git::WorkspaceGit> = Arc::new(remote::RemoteGit::new(
            remote.clone(),
            remote_cwd.unwrap_or(".").to_string(),
        ));

        let (tx, rx) = mpsc::channel::<UiEvent>();
        // The stdout reader routes `fs_result`/`git_result` lines to `remote`
        // (off the UI thread, so a blocking remote op can't deadlock); the rest
        // become UiEvents on `tx`.
        spawn_stdout_reader(stdout, tx.clone(), waker.clone(), Some(remote));
        spawn_stderr_reader(stderr, tx.clone(), waker);

        Ok((
            CodePuppy {
                child,
                stdin,
                next_id: AtomicU64::new(1),
            },
            rx,
            remote_fs,
            remote_git,
        ))
    }

    /// SSH-FALLBACK mode (see `backend::ssh_fallback`): the sidecar runs
    /// LOCALLY in a scratch cwd whose generated AGENTS.md instructs the
    /// agent to operate on the remote project via ssh commands; the GUI's
    /// fs/git ride one-shot ssh execs against the real remote files. Used
    /// when [`spawn_remote`](Self::spawn_remote) says CannotHost and the
    /// user explicitly opts in.
    #[allow(dead_code)] // consumed by the gpui shell's fallback offer flow
    pub fn spawn_ssh_fallback(
        waker: Arc<dyn UiWaker>,
        target: &ssh::SshTarget,
        remote_root: &str,
    ) -> Result<RemoteHandle, String> {
        let scratch = ssh_fallback::prepare_scratch_dir(target, remote_root)?;
        let (backend, rx) = Self::spawn(waker, Some(&scratch))?;
        let fs: Arc<dyn crate::workspace::fs::WorkspaceFs> = Arc::new(
            crate::workspace::fs::CachedFs::new(ssh_fallback::SshFs::new(target.clone())),
        );
        let git: Arc<dyn crate::git::WorkspaceGit> = Arc::new(ssh_fallback::SshGit::new(
            target.clone(),
            remote_root.to_string(),
        ));
        Ok((backend, rx, fs, git))
    }

    fn write(&self, obj: Value) {
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = writeln!(stdin, "{obj}");
            let _ = stdin.flush();
        }
    }

    /// Send a user turn. Returns the id used to correlate the eventual result.
    pub fn send_prompt(&self, text: &str, images: &[String]) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(protocol::prompt(id, text, images));
        id
    }

    /// Cancel the currently running agent turn.
    pub fn cancel(&self) {
        self.write(protocol::cancel());
    }

    /// Send a slash command (`/...`). Returns the correlation id.
    pub fn send_command(&self, text: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(protocol::command(id, text));
        id
    }

    /// Ask the sidecar to re-send the command catalog.
    #[allow(dead_code)] // protocol op mirror; catalogs currently arrive on `ready`
    pub fn list_commands(&self) {
        self.write(protocol::list_commands());
    }

    /// Ask the sidecar to re-send the agent catalog.
    #[allow(dead_code)] // protocol op mirror; catalogs currently arrive on `ready`
    pub fn list_agents(&self) {
        self.write(protocol::list_agents());
    }

    /// Ask the sidecar to re-send the model catalog.
    #[allow(dead_code)] // protocol op mirror; catalogs currently arrive on `ready`
    pub fn list_models(&self) {
        self.write(protocol::list_models());
    }

    /// Switch the active model (sidecar reloads the agent and re-announces).
    pub fn set_model(&self, name: &str) {
        self.write(protocol::set_model(name));
    }

    /// Request a live run-metrics snapshot (answered by a `Status` event).
    pub fn request_status(&self) {
        self.write(protocol::status());
    }

    /// Ask the sidecar for the catalog of saved sessions (`Sessions` event).
    pub fn list_sessions(&self) {
        self.write(protocol::list_sessions());
    }

    /// Load a saved session into the agent (`source` = "autosave" | "context").
    pub fn load_session(&self, name: &str, source: &str) {
        self.write(protocol::load_session(name, source));
    }

    /// Request a read-only preview of a session's conversation (`SessionPreview`).
    pub fn preview_session(&self, name: &str, source: &str) {
        self.write(protocol::preview_session(name, source));
    }

    /// Pause the running turn at the next safe boundary.
    pub fn pause(&self) {
        self.write(protocol::pause());
    }

    /// Resume a paused turn.
    pub fn resume(&self) {
        self.write(protocol::resume());
    }

    /// Steer the running turn: `mode` is "now" (mid-turn) or "queue" (next turn).
    pub fn steer(&self, text: &str, mode: &str) {
        self.write(protocol::steer(text, mode));
    }

    /// Request completions for `text` with the caret at char index `cursor`.
    /// Returns the correlation id (use it to ignore stale responses).
    pub fn request_completion(&self, text: &str, cursor: usize) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(protocol::complete(id, text, cursor));
        id
    }

    /// Answer an interactive question (`Ask`).
    pub fn ask_response(&self, id: &str, answers: &[AskAnswer]) {
        self.write(protocol::ask_response(id, answers));
    }

    /// Cancel an interactive question.
    pub fn ask_cancel(&self, id: &str) {
        self.write(protocol::ask_cancel(id));
    }

    pub fn respond_input(&self, prompt_id: &str, value: &str) {
        self.write(protocol::respond_input(prompt_id, value));
    }

    pub fn respond_confirmation(&self, prompt_id: &str, confirmed: bool, feedback: Option<&str>) {
        self.write(protocol::respond_confirmation(
            prompt_id, confirmed, feedback,
        ));
    }

    pub fn respond_selection(&self, prompt_id: &str, index: i64, value: &str) {
        self.write(protocol::respond_selection(prompt_id, index, value));
    }

    pub fn set_agent(&self, name: &str) {
        self.write(protocol::set_agent(name));
    }

    /// Ask the sidecar for the MCP server catalog (`McpServers` event).
    pub fn list_mcp_servers(&self) {
        self.write(protocol::list_mcp_servers());
    }

    /// Start (`true`) or stop (`false`) an MCP server; the sidecar re-lists.
    pub fn set_mcp_enabled(&self, name: &str, enabled: bool) {
        self.write(protocol::set_mcp_enabled(name, enabled));
    }

    /// Register a new MCP server; the sidecar re-lists (or emits `Error`).
    pub fn add_mcp_server(&self, name: &str, transport: &str, config: &Value) {
        self.write(protocol::add_mcp_server(name, transport, config));
    }

    /// Ask the sidecar for the skill catalog (`Skills` event).
    pub fn list_skills(&self) {
        self.write(protocol::list_skills());
    }

    /// Ask for one skill's full SKILL.md (`SkillDetail` event).
    pub fn get_skill(&self, name: &str) {
        self.write(protocol::get_skill(name));
    }

    /// Enable (`true`) or disable (`false`) a skill; the sidecar re-lists.
    pub fn set_skill_enabled(&self, name: &str, enabled: bool) {
        self.write(protocol::set_skill_enabled(name, enabled));
    }

    /// Create or overwrite a skill; the sidecar re-lists (or emits `Error`).
    pub fn save_skill(&self, name: &str, description: &str, content: &str, scope: &str) {
        self.write(protocol::save_skill(name, description, content, scope));
    }

    /// Ask the sidecar for the agent catalog (`AgentConfigs` event).
    pub fn list_agent_configs(&self) {
        self.write(protocol::list_agent_configs());
    }

    /// Ask for one agent's full config (`AgentConfigDetail` event).
    pub fn get_agent_config(&self, name: &str) {
        self.write(protocol::get_agent_config(name));
    }

    /// Create or overwrite a JSON agent; the sidecar re-lists (or emits `Error`).
    pub fn save_agent_config(&self, draft: &AgentConfigDraft) {
        self.write(protocol::save_agent_config(draft));
    }

    /// Delete a JSON agent config; the sidecar re-lists (or emits `Error`).
    pub fn delete_agent_config(&self, name: &str) {
        self.write(protocol::delete_agent_config(name));
    }

    /// Clone an agent into an editable user JSON copy; the sidecar re-lists.
    pub fn clone_agent_config(&self, name: &str) {
        self.write(protocol::clone_agent_config(name));
    }

    /// Ask for kennel-wide totals (`KennelStats` event).
    pub fn kennel_stats(&self) {
        self.write(protocol::kennel_stats());
    }

    /// Ask for every wing + drawer count (`KennelWings` event).
    pub fn kennel_list_wings(&self) {
        self.write(protocol::kennel_list_wings());
    }

    /// Ask for the most recent drawers across `wings` (`KennelDrawers` event).
    pub fn kennel_recent(&self, wings: &[String], limit: u64) {
        self.write(protocol::kennel_recent(wings, limit));
    }

    /// FTS5 search across `wings` (`KennelDrawers` event).
    pub fn kennel_search(&self, query: &str, wings: &[String], limit: u64) {
        self.write(protocol::kennel_search(query, wings, limit));
    }

    /// Ask for the judge roster (`Judges` event).
    pub fn list_judges(&self) {
        self.write(protocol::list_judges());
    }

    /// Ask for one judge's full config (`JudgeDetail` event).
    pub fn get_judge(&self, name: &str) {
        self.write(protocol::get_judge(name));
    }

    /// Create or update a judge; the sidecar re-lists (or emits `Error`).
    pub fn save_judge(&self, draft: &JudgeDraft) {
        self.write(protocol::save_judge(draft));
    }

    /// Delete a judge; the sidecar re-lists (or emits `Error`).
    pub fn delete_judge(&self, name: &str) {
        self.write(protocol::delete_judge(name));
    }

    /// Flip a judge's enabled flag; the sidecar re-lists.
    pub fn toggle_judge(&self, name: &str) {
        self.write(protocol::toggle_judge(name));
    }
}

impl Drop for CodePuppy {
    fn drop(&mut self) {
        self.write(protocol::shutdown());
        // Don't block app shutdown on a hung child.
        let _ = self.child.kill();
    }
}

fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    tx: Sender<UiEvent>,
    waker: Arc<dyn UiWaker>,
    rpc: Option<Arc<remote::RemoteState>>,
) {
    std::thread::Builder::new()
        .name("cp-stdout".into())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                // For a remote sidecar, intercept `fs_result` RPC replies and
                // hand them to the remote-fs state instead of the event stream.
                let event = if let Some(rpc) = &rpc {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(val) => {
                            if matches!(
                                val.get("event").and_then(|e| e.as_str()),
                                Some("fs_result" | "git_result")
                            ) {
                                rpc.handle_result(&val);
                                waker.wake();
                                continue;
                            }
                            match serde_json::from_value::<Wire>(val) {
                                Ok(wire) => UiEvent::from(wire),
                                Err(e) => UiEvent::Log(format!("[unparsed] {e}: {line}")),
                            }
                        }
                        Err(e) => UiEvent::Log(format!("[unparsed] {e}: {line}")),
                    }
                } else {
                    match serde_json::from_str::<Wire>(&line) {
                        Ok(wire) => UiEvent::from(wire),
                        Err(e) => UiEvent::Log(format!("[unparsed] {e}: {line}")),
                    }
                };
                if tx.send(event).is_err() {
                    break;
                }
                waker.wake();
            }
            let _ = tx.send(UiEvent::Exited { code: None });
            waker.wake();
        })
        .expect("spawn stdout reader");
}

fn spawn_stderr_reader(
    stderr: std::process::ChildStderr,
    tx: Sender<UiEvent>,
    waker: Arc<dyn UiWaker>,
) {
    std::thread::Builder::new()
        .name("cp-stderr".into())
        .spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if tx.send(UiEvent::Log(line)).is_err() {
                    break;
                }
                waker.wake();
            }
        })
        .expect("spawn stderr reader");
}

/// Write the embedded sidecar to a stable on-disk location and return its path.
fn write_sidecar() -> std::io::Result<std::path::PathBuf> {
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("sidecar.py");
    std::fs::write(&path, SIDECAR_PY)?;
    Ok(path)
}

/// Per-user data directory for puppy-home.
fn app_data_dir() -> std::path::PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        return std::path::Path::new(&base).join("puppy-home");
    }
    std::env::temp_dir().join("puppy-home")
}

/// Decide how to launch the sidecar — the "detect, else auto-provision" policy.
///
/// Order of preference:
///   1. `PUPPY_HOME_CP_CMD` override (whitespace-split; sidecar path appended).
///   2. An existing Python on PATH that can already `import code_puppy`.
///   3. `uv run --with code-puppy ...` — auto-provisions from PyPI (cached).
fn resolve_launch(sidecar: &std::path::Path) -> Result<Command, String> {
    let sidecar = sidecar.to_string_lossy().to_string();

    // 1. Explicit override.
    if let Ok(raw) = std::env::var("PUPPY_HOME_CP_CMD") {
        let parts: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
        if let Some((program, args)) = parts.split_first() {
            let mut cmd = Command::new(program);
            cmd.args(args).arg(&sidecar);
            return Ok(cmd);
        }
    }

    // 2. Detect an existing code_puppy-capable interpreter.
    for python in ["python", "python3", "py"] {
        if can_import_code_puppy(python) {
            let mut cmd = Command::new(python);
            cmd.arg(&sidecar);
            return Ok(cmd);
        }
    }

    // 3. Auto-provision with uv.
    if program_exists("uv") {
        let mut cmd = Command::new("uv");
        cmd.args(["run", "--with", "code-puppy", "python", &sidecar]);
        return Ok(cmd);
    }

    Err(
        "No Code Puppy environment found and `uv` is not installed. \
         Install uv (https://docs.astral.sh/uv/) or set PUPPY_HOME_CP_CMD."
            .to_string(),
    )
}

/// Does `<python> -c "import code_puppy"` succeed?
fn can_import_code_puppy(python: &str) -> bool {
    let mut cmd = Command::new(python);
    crate::proc::hide_console(&mut cmd);
    cmd.args(["-c", "import code_puppy"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is a program resolvable on PATH (does `<prog> --version` run)?
fn program_exists(prog: &str) -> bool {
    let mut cmd = Command::new(prog);
    crate::proc::hide_console(&mut cmd);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirror the stdout reader's decode path: JSON line -> Wire -> UiEvent.
    fn decode(line: &str) -> UiEvent {
        serde_json::from_str::<Wire>(line)
            .map(UiEvent::from)
            .unwrap_or_else(|e| panic!("decode failed: {e}: {line}"))
    }

    #[test]
    fn ready_event_roundtrips() {
        let ev = decode(
            r#"{"event":"ready","agent":"code-puppy","model":"gpt","cp_version":"1.2","cwd":"/tmp","autosave":"auto_session_x","puppy_name":"Rufus","owner_name":"Jacob"}"#,
        );
        match ev {
            UiEvent::Ready {
                agent,
                model,
                puppy_name,
                owner_name,
                ..
            } => {
                assert_eq!(agent, "code-puppy");
                assert_eq!(model, "gpt");
                assert_eq!(puppy_name, "Rufus");
                assert_eq!(owner_name, "Jacob");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn message_event_carries_payload() {
        let ev = decode(
            r#"{"event":"message","source":"bus","kind":"DiffMessage","category":"tool_output","text":"edited","payload":{"path":"a.rs"}}"#,
        );
        match ev {
            UiEvent::Message(m) => {
                assert_eq!(m.kind, "DiffMessage");
                assert_eq!(m.category, "tool_output");
                assert_eq!(m.payload["path"], "a.rs");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn result_event() {
        match decode(r#"{"event":"result","id":7,"output":"done"}"#) {
            UiEvent::Result { id, output } => {
                assert_eq!(id, 7);
                assert_eq!(output, "done");
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn error_event_allows_null_id() {
        match decode(r#"{"event":"error","id":null,"message":"boom"}"#) {
            UiEvent::Error { id, message } => {
                assert_eq!(id, None);
                assert_eq!(message, "boom");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn paused_event() {
        assert!(matches!(
            decode(r#"{"event":"paused","paused":true}"#),
            UiEvent::Paused(true)
        ));
    }

    #[test]
    fn status_event_with_sub_agents() {
        let ev = decode(
            r#"{"event":"status","stats":"3 msgs","token_rate":12.5,"sub_agents":[{"agent_name":"helper","status":"running"}]}"#,
        );
        match ev {
            UiEvent::Status {
                stats,
                token_rate,
                sub_agents,
                // A pre-control-surface sidecar omits these: defaults apply.
                paused,
                queued,
                last_prompt,
                total_tokens,
                ctx_pct,
                ctx,
                cost,
                cost_estimated,
            } => {
                assert_eq!(stats, "3 msgs");
                assert!((token_rate - 12.5).abs() < f64::EPSILON);
                assert_eq!(sub_agents.len(), 1);
                assert_eq!(sub_agents[0].agent_name, "helper");
                assert!(!paused);
                assert_eq!(queued, 0);
                assert_eq!(last_prompt, "");
                assert_eq!(total_tokens, 0);
                // Phase-F fields also default cleanly on old payloads.
                assert_eq!(ctx_pct, None);
                assert_eq!(ctx, None);
                assert_eq!(cost, None);
                assert!(!cost_estimated);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn status_event_with_control_surface_fields() {
        let ev = decode(
            r#"{"event":"status","stats":"","token_rate":0.0,"sub_agents":[],"paused":true,"queued":2,"last_prompt":"fix the tests","total_tokens":12345,"cost":null}"#,
        );
        match ev {
            UiEvent::Status {
                paused,
                queued,
                last_prompt,
                total_tokens,
                cost,
                ..
            } => {
                assert!(paused);
                assert_eq!(queued, 2);
                assert_eq!(last_prompt, "fix the tests");
                assert_eq!(total_tokens, 12345);
                assert_eq!(cost, None);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn status_event_with_ctx_and_estimated_cost() {
        // Phase-F sidecar: ctx-% present, cost priced from the bundled
        // models.dev snapshot with the estimate flag riding along.
        let ev = decode(
            r#"{"event":"status","stats":"","token_rate":0.0,"sub_agents":[],"total_tokens":50000,"ctx_pct":37.5,"cost":0.1234,"cost_estimated":true}"#,
        );
        match ev {
            UiEvent::Status {
                ctx_pct,
                cost,
                cost_estimated,
                ..
            } => {
                assert_eq!(ctx_pct, Some(37.5));
                assert_eq!(cost, Some(0.1234));
                assert!(cost_estimated);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn status_event_parses_ctx_breakdown() {
        // The /context popover's data: buckets + capacity + threshold.
        let ev = decode(
            r#"{"event":"status","stats":"","token_rate":0.0,"sub_agents":[],"ctx_pct":42.0,"ctx":{"percent":42.0,"used_tokens":1000,"overhead_tokens":3000,"total_tokens":4000,"capacity":10000,"system_prompt_tokens":1500,"agents_md_tokens":500,"pydantic_tools_tokens":700,"mcp_tokens":300,"kennel_memory_tokens":0,"compaction_threshold":0.85}}"#,
        );
        match ev {
            UiEvent::Status { ctx, .. } => {
                let c = ctx.expect("breakdown present");
                assert_eq!(c.used_tokens, 1000);
                assert_eq!(c.overhead_tokens, 3000);
                assert_eq!(c.capacity, 10000);
                assert_eq!(c.system_prompt_tokens, 1500);
                assert_eq!(c.pydantic_tools_tokens, 700);
                assert!((c.compaction_threshold - 0.85).abs() < f64::EPSILON);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn status_event_ctx_pct_null_is_unknown_not_zero() {
        // null must survive as None — a 0% bar would be a lie.
        let ev = decode(
            r#"{"event":"status","stats":"","token_rate":0.0,"sub_agents":[],"ctx_pct":null,"cost":null,"cost_estimated":true}"#,
        );
        match ev {
            UiEvent::Status {
                ctx_pct,
                cost,
                cost_estimated,
                ..
            } => {
                assert_eq!(ctx_pct, None);
                assert_eq!(cost, None);
                assert!(cost_estimated);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn completions_correlate_by_id() {
        let ev = decode(
            r#"{"event":"completions","id":42,"items":[{"text":"/help","display":"/help"}]}"#,
        );
        match ev {
            UiEvent::Completions { id, items } => {
                assert_eq!(id, 42);
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].text, "/help");
            }
            other => panic!("expected Completions, got {other:?}"),
        }
    }

    #[test]
    fn sessions_event_defaults_optional_fields() {
        let ev = decode(r#"{"event":"sessions","items":[{"name":"auto_session_1"}]}"#);
        match ev {
            UiEvent::Sessions {
                items,
                current,
                open,
            } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "auto_session_1");
                assert_eq!(current, "");
                assert!(!open);
            }
            other => panic!("expected Sessions, got {other:?}"),
        }
    }

    #[test]
    fn ask_event_questions() {
        let ev = decode(
            r#"{"event":"ask","id":"q1","questions":[{"header":"Pick","question":"which?","options":[{"label":"A"},{"label":"B"}]}]}"#,
        );
        match ev {
            UiEvent::Ask { id, questions } => {
                assert_eq!(id, "q1");
                assert_eq!(questions.len(), 1);
                assert_eq!(questions[0].options.len(), 2);
            }
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn ask_answer_serializes_for_the_wire() {
        let a = AskAnswer {
            question_header: "Pick".into(),
            selected_options: vec!["A".into()],
            other_text: None,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["question_header"], "Pick");
        assert_eq!(v["selected_options"][0], "A");
        assert!(v["other_text"].is_null());
    }

    #[test]
    fn mcp_servers_event() {
        let ev = decode(
            r#"{"event":"mcp_servers","items":[{"id":"abc","name":"filesystem","type":"stdio","enabled":true,"state":"running","summary":"npx -y server","error":""}]}"#,
        );
        match ev {
            UiEvent::McpServers(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "filesystem");
                assert_eq!(items[0].transport, "stdio");
                assert!(items[0].enabled);
                assert_eq!(items[0].state, "running");
                assert_eq!(items[0].summary, "npx -y server");
                assert_eq!(items[0].error, "");
            }
            other => panic!("expected McpServers, got {other:?}"),
        }
    }

    #[test]
    fn mcp_servers_event_defaults_optional_fields() {
        match decode(r#"{"event":"mcp_servers","items":[{"name":"bare"}]}"#) {
            UiEvent::McpServers(items) => {
                assert_eq!(items[0].name, "bare");
                assert_eq!(items[0].transport, "");
                assert!(!items[0].enabled);
                assert_eq!(items[0].state, "");
            }
            other => panic!("expected McpServers, got {other:?}"),
        }
    }

    #[test]
    fn skills_event() {
        let ev = decode(
            r#"{"event":"skills","items":[{"name":"git-flow","description":"Release flow","path":"C:/u/.code_puppy/skills/git-flow","enabled":true,"source":"user"}]}"#,
        );
        match ev {
            UiEvent::Skills(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "git-flow");
                assert_eq!(items[0].description, "Release flow");
                assert_eq!(items[0].path, "C:/u/.code_puppy/skills/git-flow");
                assert!(items[0].enabled);
                assert_eq!(items[0].source, "user");
            }
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn skills_event_defaults_optional_fields() {
        match decode(r#"{"event":"skills","items":[{"name":"bare"}]}"#) {
            UiEvent::Skills(items) => {
                assert_eq!(items[0].name, "bare");
                assert_eq!(items[0].description, "");
                assert!(!items[0].enabled);
                assert_eq!(items[0].source, "");
            }
            other => panic!("expected Skills, got {other:?}"),
        }
    }

    #[test]
    fn skill_detail_event() {
        let ev = decode(
            r#"{"event":"skill_detail","name":"git-flow","description":"Release flow","path":"/s/git-flow","content":"---\nname: git-flow\n---\nBody"}"#,
        );
        match ev {
            UiEvent::SkillDetail(d) => {
                assert_eq!(d.name, "git-flow");
                assert_eq!(d.description, "Release flow");
                assert_eq!(d.path, "/s/git-flow");
                assert!(d.content.starts_with("---"));
            }
            other => panic!("expected SkillDetail, got {other:?}"),
        }
    }

    #[test]
    fn kennel_stats_event() {
        match decode(r#"{"event":"kennel_stats","drawers":123,"wings":3,"bytes":557056}"#) {
            UiEvent::KennelStats(s) => {
                assert_eq!(s.drawers, 123);
                assert_eq!(s.wings, 3);
                assert_eq!(s.bytes, 557056);
            }
            other => panic!("expected KennelStats, got {other:?}"),
        }
    }

    #[test]
    fn kennel_wings_event() {
        match decode(
            r#"{"event":"kennel_wings","items":[{"name":"repo:/x","count":118},{"name":"user:default"}]}"#,
        ) {
            UiEvent::KennelWings(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].name, "repo:/x");
                assert_eq!(items[0].count, 118);
                assert_eq!(items[1].name, "user:default");
                assert_eq!(items[1].count, 0); // defaulted
            }
            other => panic!("expected KennelWings, got {other:?}"),
        }
    }

    #[test]
    fn kennel_drawers_event_lifts_metadata() {
        match decode(
            r#"{"event":"kennel_drawers","items":[{"id":7,"role":"note","content":"hi","ts":"2026-06-15T17:19:57+00:00","session_id":"s1","agent":"code-puppy","cwd":"/x"}]}"#,
        ) {
            UiEvent::KennelDrawers(items) => {
                assert_eq!(items.len(), 1);
                let d = &items[0];
                assert_eq!(d.id, 7);
                assert_eq!(d.role, "note");
                assert_eq!(d.content, "hi");
                assert_eq!(d.agent, "code-puppy");
                assert_eq!(d.cwd, "/x");
            }
            other => panic!("expected KennelDrawers, got {other:?}"),
        }
    }

    #[test]
    fn judges_event() {
        match decode(
            r#"{"event":"judges","items":[{"name":"tests-pass","model":"gpt-5","prompt":"check","enabled":true}]}"#,
        ) {
            UiEvent::Judges(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "tests-pass");
                assert_eq!(items[0].model, "gpt-5");
                assert!(items[0].enabled);
            }
            other => panic!("expected Judges, got {other:?}"),
        }
    }

    #[test]
    fn judge_detail_event() {
        match decode(
            r#"{"event":"judge_detail","name":"docs","model":"gpt-5","prompt":"verify docs","enabled":false}"#,
        ) {
            UiEvent::JudgeDetail(d) => {
                assert_eq!(d.name, "docs");
                assert_eq!(d.model, "gpt-5");
                assert_eq!(d.prompt, "verify docs");
                assert!(!d.enabled);
            }
            other => panic!("expected JudgeDetail, got {other:?}"),
        }
    }

    #[test]
    fn unknown_event_tag_fails_to_parse() {
        assert!(serde_json::from_str::<Wire>(r#"{"event":"nonsense"}"#).is_err());
    }
}
