//! Data-transfer types for the Code Puppy protocol: the structs the GUI reads
//! off the wire (commands, agents, models, sessions, skills, ask prompts, ...).
//! Split out of `backend/mod.rs` (G5 hygiene); the wire decode + process
//! driver stay there.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[allow(dead_code)] // part of the wire contract; not yet surfaced in the UI
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
    #[allow(dead_code)] // wire contract; not yet surfaced in the UI
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

/// A saved Code Puppy session (autosave conversation or named context).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    #[serde(default)]
    pub source: String, // "autosave" | "context"
    #[serde(default)]
    #[allow(dead_code)] // part of the wire contract; not yet surfaced in the UI
    pub timestamp: String,
    #[serde(default)]
    pub messages: u64,
    #[serde(default)]
    #[allow(dead_code)] // wire contract; not yet surfaced in the UI
    pub tokens: u64,
}

/// One reconstructed transcript row from a loaded session.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionEntry {
    #[serde(default)]
    pub role: String, // "user" | "agent"
    #[serde(default)]
    pub text: String,
}

/// A registered MCP server (from Code Puppy's MCP manager, global config).
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    #[allow(dead_code)] // registry id; ops address servers by name
    pub id: String,
    pub name: String,
    /// Transport: "stdio" | "sse" | "http".
    #[serde(rename = "type", default)]
    pub transport: String,
    #[serde(default)]
    pub enabled: bool,
    /// Lifecycle state: "running" | "starting" | "stopped" | "stopping" |
    /// "error" | "quarantined".
    #[serde(default)]
    pub state: String,
    /// One-line config summary: the command line (stdio) or the URL.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub error: String,
}

/// A discovered Code Puppy skill (agent_skills plugin: a SKILL.md on disk).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub enabled: bool,
    /// Where the skill lives: "user" | "project" | "plugin" | "other".
    #[serde(default)]
    pub source: String,
}

/// One skill's full SKILL.md text (answer to `get_skill`).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillDetail {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: String,
}

/// One agent in the catalog (answer to `list_agent_configs`). JSON agents are
/// `editable`; built-in Python agents are read-only (clone them to edit).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfigInfo {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub tool_count: u64,
    /// Where it lives: "user" | "project" | "builtin".
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub path: String,
    /// True when this is the workspace's active agent (can't be deleted).
    #[serde(default)]
    pub current: bool,
}

/// One agent's full config (answer to `get_agent_config`).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfigDetail {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    /// Optional custom user prompt (absent for most agents).
    #[serde(default)]
    pub user_prompt: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub path: String,
    /// The pretty-printed JSON that lands on disk (review pane).
    #[serde(default)]
    pub content: String,
}

/// A draft agent config the visual builder sends to `save_agent_config`.
/// Empty `model`/`user_prompt` are omitted from the on-disk JSON by the sidecar.
#[derive(Debug, Clone, Default)]
pub struct AgentConfigDraft {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub model: String,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<String>,
    /// "user" or "project".
    pub scope: String,
}

/// Kennel-wide totals (answer to `kennel_stats`): drawer + wing counts and
/// the on-disk SQLite size in bytes. All global — the kennel DB is shared.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct KennelStats {
    #[serde(default)]
    pub drawers: u64,
    #[serde(default)]
    pub wings: u64,
    #[serde(default)]
    pub bytes: u64,
}

/// One kennel wing with its drawer count (answer to `kennel_list_wings`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KennelWing {
    pub name: String,
    #[serde(default)]
    pub count: u64,
}

/// A single remembered drawer (answer rows of `kennel_recent`/`kennel_search`).
/// `agent`/`cwd` are lifted from the drawer metadata (mirrors
/// `tools._drawer_to_model`). Read-only in the GUI.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KennelDrawer {
    pub id: i64,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub cwd: String,
}

/// One configured goal-mode judge (answer rows of `list_judges`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeInfo {
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub enabled: bool,
}

/// One judge's full config (answer to `get_judge`). Same shape as `JudgeInfo`
/// today, kept distinct so the detail event can grow independently.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeDetail {
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub enabled: bool,
}

/// A draft the judge builder sends to `save_judge`. `is_new` adds a judge;
/// otherwise `name` is updated in place (`new_name` renames). An empty prompt
/// is replaced with the standard goal-judge prompt by the sidecar.
#[derive(Debug, Clone, Default)]
pub struct JudgeDraft {
    pub name: String,
    pub new_name: String,
    pub model: String,
    pub prompt: String,
    pub enabled: bool,
    pub is_new: bool,
}

/// The live goal-loop HUD feed (`goal_state` — emitted on every change).
/// The sidecar emulates the CLI's goal loop; `mode` is "goal" while active.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct GoalStateMsg {
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub prompt: String,
    #[serde(rename = "loop", default)]
    pub loop_count: u64,
    #[serde(default)]
    pub max: u64,
    #[serde(default)]
    pub mode: String,
}

/// One judging round's result (`goal_iteration`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GoalIterationMsg {
    #[serde(rename = "loop", default)]
    pub loop_count: u64,
    #[serde(default)]
    pub max: u64,
    #[serde(default)]
    pub all_complete: bool,
    #[serde(default)]
    pub remediation_notes: String,
}

/// The goal loop finished (`goal_done`). `reason` is
/// "all_pass" | "max_iters" | "stopped".
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GoalDoneMsg {
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub loops: u64,
    #[serde(default)]
    pub reason: String,
}

/// One judge in a round's roster (rows of `judge_run_started.judges`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeRosterEntry {
    pub name: String,
    #[serde(default)]
    pub model: String,
}

/// A judging round began (`judge_run_started`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeRunStarted {
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub iteration: u64,
    #[serde(default)]
    pub max: u64,
    #[serde(default)]
    pub judges: Vec<JudgeRosterEntry>,
}

/// One judge began running (`judge_started`) — drives the "running" row state.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeStartedMsg {
    pub judge_name: String,
    #[serde(default)]
    pub iteration: u64,
}

/// One judge's verdict (`judge_verdict`) — resolves a live row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeVerdictMsg {
    pub judge_name: String,
    #[serde(default)]
    pub iteration: u64,
    #[serde(default)]
    pub complete: bool,
    #[serde(default)]
    pub abstained: bool,
    #[serde(default)]
    pub notes: String,
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
    #[allow(dead_code)] // wire contract; not yet surfaced in the UI
    pub token_count: u64,
    #[serde(default)]
    pub current_tool: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // wire contract; not yet surfaced in the UI
    pub elapsed: f64,
}

/// The `/context` breakdown the sidecar forwards on each status tick:
/// conversation history (`used_tokens`) + fixed overhead (split into the
/// system-prompt / AGENTS.md / tools / MCP / kennel buckets) over the
/// model's `capacity`, plus the compaction-trigger threshold. All `None`
/// (absent) when code_puppy can't estimate — honesty over a fake zero.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ContextBreakdown {
    #[serde(default)]
    pub percent: f64,
    #[serde(default)]
    pub used_tokens: u64,
    #[serde(default)]
    pub overhead_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub capacity: u64,
    #[serde(default)]
    pub system_prompt_tokens: u64,
    #[serde(default)]
    pub agents_md_tokens: u64,
    #[serde(default)]
    pub pydantic_tools_tokens: u64,
    #[serde(default)]
    pub mcp_tokens: u64,
    #[serde(default)]
    pub kennel_memory_tokens: u64,
    /// Compaction fires at this fraction of capacity (default 0.85).
    #[serde(default)]
    pub compaction_threshold: f64,
}

/// One selectable option in an interactive question.
#[derive(Debug, Clone, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    #[allow(dead_code)] // wire contract; not yet surfaced in the UI
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
    #[allow(dead_code)] // which Code Puppy messaging system emitted this
    pub source: String,
    pub kind: String,
    pub category: String,
    pub text: String,
    pub payload: Value,
}
