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
use std::sync::Mutex;

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The sidecar source, embedded so the executable is self-contained.
const SIDECAR_PY: &str = include_str!("../../sidecar/sidecar.py");

/// Metadata for a Code Puppy slash command (drives the commands menu).
#[derive(Debug, Clone, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)]
    pub usage: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A single completion candidate (mirrors a prompt_toolkit `Completion`).
#[derive(Debug, Clone, Deserialize)]
pub struct CompletionItem {
    /// Text to insert.
    #[serde(default)]
    pub text: String,
    /// Caret-relative offset (≤ 0) where the insert begins — how many chars
    /// before the cursor to replace.
    #[serde(default)]
    pub start_position: i64,
    /// What to show in the list.
    #[serde(default)]
    pub display: String,
    /// Right-hand hint (e.g. description, current value, file type).
    #[serde(default)]
    pub meta: String,
}

/// An available agent (for the agent picker).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub current: bool,
}

/// An available model (for the model picker).
#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub current: bool,
}

/// A concurrent sub-agent Code Puppy spawned via `invoke_agent` (dashboard row).
#[derive(Debug, Clone, Deserialize)]
pub struct SubAgentInfo {
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tool_call_count: u64,
    #[serde(default)]
    pub token_count: u64,
    #[serde(default)]
    pub current_tool: Option<String>,
    #[serde(default)]
    pub elapsed: f64,
}

/// One selectable option in an interactive question.
#[derive(Debug, Clone, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A single interactive question (from Code Puppy's `ask_user_question` tool).
#[derive(Debug, Clone, Deserialize)]
pub struct AskQuestion {
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub options: Vec<AskOption>,
}

/// An answer the GUI sends back for one question.
#[derive(Debug, Clone, Serialize)]
pub struct AskAnswer {
    pub question_header: String,
    pub selected_options: Vec<String>,
    pub other_text: Option<String>,
}

/// A structured message forwarded from one of Code Puppy's messaging systems.
#[derive(Debug, Clone)]
pub struct BackendMessage {
    pub source: String,
    pub kind: String,
    pub category: String,
    pub text: String,
    pub payload: Value,
}

/// Events flowing from the backend to the GUI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Sidecar initialized and the agent is ready (also re-sent on agent/model switch).
    Ready {
        agent: String,
        model: String,
        cp_version: String,
        cwd: String,
    },
    /// A streamed message from the agent (assistant text, tool output, etc.).
    Message(BackendMessage),
    /// The catalog of available slash commands.
    Commands(Vec<CommandInfo>),
    /// The catalog of available agents.
    Agents(Vec<AgentInfo>),
    /// The catalog of available models.
    Models(Vec<ModelInfo>),
    /// Completion candidates for a `request_completion` (correlated by `id`).
    Completions { id: u64, items: Vec<CompletionItem> },
    /// An interactive question — answer via `ask_response` / `ask_cancel`.
    Ask { id: String, questions: Vec<AskQuestion> },
    /// A user turn finished; `output` is the agent's final response.
    Result { id: u64, output: String },
    /// A slash command finished (`handled` = whether the dispatcher claimed it).
    CommandDone { id: u64, handled: bool },
    /// An error tied to a turn (`id`) or the bridge itself (`id = None`).
    Error { id: Option<u64>, message: String },
    /// Diagnostic log line (sidecar stderr or internal notices).
    Log(String),
    /// Live run metrics snapshot (answer to `request_status`).
    Status {
        stats: String,
        token_rate: f64,
        sub_agents: Vec<SubAgentInfo>,
    },
    /// The running turn was paused (`true`) or resumed (`false`).
    Paused(bool),
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
    Agents {
        #[serde(default)]
        items: Vec<AgentInfo>,
    },
    Models {
        #[serde(default)]
        items: Vec<ModelInfo>,
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
    },
    Paused {
        #[serde(default)]
        paused: bool,
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
            } => UiEvent::Ready {
                agent,
                model,
                cp_version,
                cwd,
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
            Wire::Agents { items } => UiEvent::Agents(items),
            Wire::Models { items } => UiEvent::Models(items),
            Wire::Completions { id, items } => UiEvent::Completions { id, items },
            Wire::Ask { id, questions } => UiEvent::Ask { id, questions },
            Wire::Result { id, output } => UiEvent::Result { id, output },
            Wire::CommandDone { id, handled } => UiEvent::CommandDone { id, handled },
            Wire::Error { id, message } => UiEvent::Error { id, message },
            Wire::Log { text } => UiEvent::Log(text),
            Wire::Status { stats, token_rate, sub_agents } => UiEvent::Status {
                stats,
                token_rate,
                sub_agents,
            },
            Wire::Paused { paused } => UiEvent::Paused(paused),
        }
    }
}

/// A running Code Puppy sidecar we can drive.
pub struct CodePuppy {
    child: Child,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
}

impl CodePuppy {
    /// Launch the sidecar. Returns the handle and the event stream, or an error
    /// string if the process couldn't even be started.
    ///
    /// `ctx` is used to wake the GUI (`request_repaint`) when events arrive.
    /// `cwd`, when set, becomes the sidecar's working directory — Code Puppy's
    /// `os.getcwd()` adopts it, scoping the whole process to that workspace.
    pub fn spawn(
        ctx: egui::Context,
        cwd: Option<&std::path::Path>,
    ) -> Result<(Self, Receiver<UiEvent>), String> {
        let sidecar_path = write_sidecar().map_err(|e| format!("writing sidecar: {e}"))?;

        let mut command = resolve_launch(&sidecar_path)?;
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

        // stdout -> protocol events
        spawn_stdout_reader(stdout, tx.clone(), ctx.clone());
        // stderr -> log events
        spawn_stderr_reader(stderr, tx.clone(), ctx.clone());

        Ok((
            CodePuppy {
                child,
                stdin: Mutex::new(stdin),
                next_id: AtomicU64::new(1),
            },
            rx,
        ))
    }

    fn write(&self, obj: Value) {
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = writeln!(stdin, "{obj}");
            let _ = stdin.flush();
        }
    }

    /// Send a user turn. Returns the id used to correlate the eventual result.
    pub fn send_prompt(&self, text: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(json!({"op": "prompt", "id": id, "text": text}));
        id
    }

    /// Cancel the currently running agent turn.
    pub fn cancel(&self) {
        self.write(json!({"op": "cancel"}));
    }

    /// Send a slash command (`/...`). Returns the correlation id.
    pub fn send_command(&self, text: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(json!({"op": "command", "id": id, "text": text}));
        id
    }

    /// Ask the sidecar to re-send the command catalog.
    pub fn list_commands(&self) {
        self.write(json!({"op": "list_commands"}));
    }

    /// Ask the sidecar to re-send the agent catalog.
    pub fn list_agents(&self) {
        self.write(json!({"op": "list_agents"}));
    }

    /// Ask the sidecar to re-send the model catalog.
    pub fn list_models(&self) {
        self.write(json!({"op": "list_models"}));
    }

    /// Switch the active model (sidecar reloads the agent and re-announces).
    pub fn set_model(&self, name: &str) {
        self.write(json!({"op": "set_model", "name": name}));
    }

    /// Request a live run-metrics snapshot (answered by a `Status` event).
    pub fn request_status(&self) {
        self.write(json!({"op": "status"}));
    }

    /// Pause the running turn at the next safe boundary.
    pub fn pause(&self) {
        self.write(json!({"op": "pause"}));
    }

    /// Resume a paused turn.
    pub fn resume(&self) {
        self.write(json!({"op": "resume"}));
    }

    /// Steer the running turn: `mode` is "now" (mid-turn) or "queue" (next turn).
    pub fn steer(&self, text: &str, mode: &str) {
        self.write(json!({"op": "steer", "text": text, "mode": mode}));
    }

    /// Request completions for `text` with the caret at char index `cursor`.
    /// Returns the correlation id (use it to ignore stale responses).
    pub fn request_completion(&self, text: &str, cursor: usize) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write(json!({"op": "complete", "id": id, "text": text, "cursor": cursor}));
        id
    }

    /// Answer an interactive question (`Ask`).
    pub fn ask_response(&self, id: &str, answers: &[AskAnswer]) {
        let answers = serde_json::to_value(answers).unwrap_or(Value::Array(vec![]));
        self.write(json!({
            "op": "ask_response", "id": id, "cancelled": false, "answers": answers,
        }));
    }

    /// Cancel an interactive question.
    pub fn ask_cancel(&self, id: &str) {
        self.write(json!({"op": "ask_response", "id": id, "cancelled": true}));
    }

    pub fn respond_input(&self, prompt_id: &str, value: &str) {
        self.write(json!({"op": "respond_input", "prompt_id": prompt_id, "value": value}));
    }

    pub fn respond_confirmation(&self, prompt_id: &str, confirmed: bool, feedback: Option<&str>) {
        self.write(json!({
            "op": "respond_confirmation",
            "prompt_id": prompt_id,
            "confirmed": confirmed,
            "feedback": feedback,
        }));
    }

    pub fn respond_selection(&self, prompt_id: &str, index: i64, value: &str) {
        self.write(json!({
            "op": "respond_selection",
            "prompt_id": prompt_id,
            "selected_index": index,
            "selected_value": value,
        }));
    }

    pub fn set_agent(&self, name: &str) {
        self.write(json!({"op": "set_agent", "name": name}));
    }
}

impl Drop for CodePuppy {
    fn drop(&mut self) {
        self.write(json!({"op": "shutdown"}));
        // Don't block app shutdown on a hung child.
        let _ = self.child.kill();
    }
}

fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    tx: Sender<UiEvent>,
    ctx: egui::Context,
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
                let event = match serde_json::from_str::<Wire>(&line) {
                    Ok(wire) => UiEvent::from(wire),
                    Err(e) => UiEvent::Log(format!("[unparsed] {e}: {line}")),
                };
                if tx.send(event).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            let _ = tx.send(UiEvent::Exited { code: None });
            ctx.request_repaint();
        })
        .expect("spawn stdout reader");
}

fn spawn_stderr_reader(
    stderr: std::process::ChildStderr,
    tx: Sender<UiEvent>,
    ctx: egui::Context,
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
                ctx.request_repaint();
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

    Err("No Code Puppy environment found and `uv` is not installed. \
         Install uv (https://docs.astral.sh/uv/) or set PUPPY_HOME_CP_CMD."
        .to_string())
}

/// Does `<python> -c "import code_puppy"` succeed?
fn can_import_code_puppy(python: &str) -> bool {
    Command::new(python)
        .args(["-c", "import code_puppy"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is a program resolvable on PATH (does `<prog> --version` run)?
fn program_exists(prog: &str) -> bool {
    Command::new(prog)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
