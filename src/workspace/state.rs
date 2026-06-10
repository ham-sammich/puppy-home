//! Shared data types for a workspace + the derived [`InstanceStatus`] and a few
//! pure helpers (pending-request parsing, tool naming).

use std::path::PathBuf;

use eframe::egui;
use serde_json::Value;

use crate::backend::BackendMessage;

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
pub(crate) enum Entry {
    User(String),
    Agent(String),
    Message(BackendMessage),
    Note(String),
    Error(String),
    /// The agent's streamed reasoning/thinking (coalesced live). `collapse` is a
    /// one-shot signal to fold the section once the turn completes.
    Thinking {
        text: String,
        collapse: std::cell::Cell<bool>,
    },
}

/// An outstanding interactive request from the agent.
pub(crate) struct Pending {
    pub(crate) prompt_id: String,
    pub(crate) kind: PendingKind,
    pub(crate) text: String,
    pub(crate) selection: usize,
}

pub(crate) enum PendingKind {
    Input {
        prompt: String,
        password: bool,
    },
    Confirm {
        title: String,
        description: String,
        options: Vec<String>,
    },
    Select {
        prompt: String,
        options: Vec<String>,
    },
}

/// An open file's editable contents.
pub(crate) struct FileBuffer {
    pub(crate) content: String,
    pub(crate) dirty: bool,
    pub(crate) load_error: Option<String>,
    pub(crate) save_error: Option<String>,
}

/// A pending rename of a tree path, edited in a modal.
pub(crate) struct PendingRename {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) error: Option<String>,
}

/// A pending "new file/folder" inside `parent`, edited in a modal.
pub(crate) struct PendingNew {
    pub(crate) parent: PathBuf,
    pub(crate) is_dir: bool,
    pub(crate) name: String,
    pub(crate) error: Option<String>,
}

/// Where the editor area (files / git / browser) sits relative to the chat.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EditorSide {
    /// Stacked: editor on top, chat in a bottom panel (the default).
    #[default]
    Bottom,
    /// Side by side: editor on the right, chat fills the left.
    Right,
}

/// A tab in the workspace's editor area (above the chat).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum EditorItem {
    Changes,
    File(PathBuf),
    /// An embedded browser tab (the browser plugin), living in this workspace.
    Browser(crate::browser::BrowserId),
    /// The Source Control / Git page (branch, staging, history).
    Git,
    /// A single commit's patch (opened from the history list).
    Commit {
        hash: String,
        short: String,
        subject: String,
    },
}

/// Cached snapshot for the Git page (refreshed on demand / after git actions).
pub(crate) struct GitView {
    pub(crate) branch: String,
    pub(crate) upstream: bool,
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
    pub(crate) staged: Vec<crate::git::GitStatusEntry>,
    pub(crate) unstaged: Vec<crate::git::GitStatusEntry>,
    pub(crate) log: Vec<crate::git::Commit>,
}

/// Directories never shown in the file tree (noisy / huge).
pub(crate) const TREE_IGNORE: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
    ".idea",
    ".vscode",
];

pub(crate) fn parse_pending(msg: &BackendMessage) -> Option<Pending> {
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
    Some(Pending {
        prompt_id,
        kind,
        text: String::new(),
        selection: 0,
    })
}

pub(crate) fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn str_vec(v: &Value, key: &str) -> Option<Vec<String>> {
    v.get(key)?.as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

/// A short, friendly tool name derived from a tool-output message kind.
pub(crate) fn tool_label(kind: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendMessage;
    use serde_json::json;

    #[test]
    fn tool_label_maps_known_kinds() {
        assert_eq!(tool_label("DiffMessage"), "edit");
        assert_eq!(tool_label("GrepResultMessage"), "grep");
        assert_eq!(tool_label("ShellOutputMessage"), "shell");
        assert_eq!(tool_label("SubAgentResponseMessage"), "sub-agent");
    }

    #[test]
    fn tool_label_passes_through_unknown() {
        assert_eq!(tool_label("MysteryMessage"), "MysteryMessage");
    }

    fn req(kind: &str, payload: serde_json::Value) -> BackendMessage {
        BackendMessage {
            source: String::new(),
            kind: kind.into(),
            category: "user_interaction".into(),
            text: String::new(),
            payload,
        }
    }

    #[test]
    fn parse_pending_confirmation() {
        let m = req(
            "ConfirmationRequest",
            json!({
                "prompt_id": "p1",
                "title": "Proceed?",
                "description": "are you sure",
                "options": ["Yes", "No"]
            }),
        );
        let p = parse_pending(&m).expect("pending");
        assert_eq!(p.prompt_id, "p1");
        match p.kind {
            PendingKind::Confirm { title, options, .. } => {
                assert_eq!(title, "Proceed?");
                assert_eq!(options, vec!["Yes".to_string(), "No".to_string()]);
            }
            _ => panic!("expected confirm"),
        }
    }

    #[test]
    fn parse_pending_input_detects_password() {
        let m = req(
            "UserInputRequest",
            json!({
                "prompt_id": "p2",
                "prompt_text": "Token:",
                "input_type": "password"
            }),
        );
        match parse_pending(&m).unwrap().kind {
            PendingKind::Input { prompt, password } => {
                assert_eq!(prompt, "Token:");
                assert!(password);
            }
            _ => panic!("expected input"),
        }
    }

    #[test]
    fn parse_pending_requires_prompt_id() {
        assert!(parse_pending(&req("UserInputRequest", json!({}))).is_none());
    }

    #[test]
    fn str_helpers_read_fields_and_arrays() {
        let v = json!({"a": "x", "list": ["1", "2"]});
        assert_eq!(str_field(&v, "a").as_deref(), Some("x"));
        assert_eq!(str_field(&v, "missing"), None);
        assert_eq!(
            str_vec(&v, "list"),
            Some(vec!["1".to_string(), "2".to_string()])
        );
    }
}
