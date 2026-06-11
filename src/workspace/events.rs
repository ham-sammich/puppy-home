//! Folding incoming [`UiEvent`]s / [`BackendMessage`]s into workspace state.
//!
//! Split out of `mod.rs`: `apply_event` is the big match over every sidecar
//! event, `on_message` classifies streamed chat messages, and the small
//! helpers (`set_status`, `enforce_transcript_cap`, `collapse_thinking`) keep
//! the derived status + bounded transcript consistent.

use std::time::Instant;

use crate::backend::{BackendMessage, UiEvent};

use super::Workspace;
use super::ask::AskState;
use super::diff::parse_diff;
use super::render::short_session;
use super::state::{Entry, InstanceStatus, parse_pending, tool_label};

impl Workspace {
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
                self.status_line = format!("Ready \u{b7} {} \u{b7} {}", self.agent, self.model);
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
                self.transcript.push(Entry::Note(format!(
                    "\u{1f436} asked: {}",
                    headers.join(", ")
                )));
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
            UiEvent::Log(line) => {
                self.logs.push(line);
                // Keep the diagnostics buffer bounded (oldest dropped).
                if self.logs.len() > 1200 {
                    self.logs.drain(..self.logs.len() - 1000);
                }
            }
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
                    "\u{27f2} Resumed session {} ({messages} messages)",
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
            UiEvent::McpServers(items) => {
                self.mcp_servers = Some(items);
                self.mcp_generation += 1;
            }
            UiEvent::Skills(items) => {
                self.skills = Some(items);
                self.skills_generation += 1;
            }
            UiEvent::SkillDetail(detail) => {
                self.skill_detail = Some(detail);
            }
            UiEvent::AgentConfigs {
                items,
                available_tools,
                available_mcp,
            } => {
                self.agent_configs = Some(items);
                self.agent_tool_catalog = available_tools;
                self.agent_mcp_catalog = available_mcp;
                self.agent_configs_generation += 1;
            }
            UiEvent::AgentConfigDetail(detail) => {
                self.agent_config_detail = Some(detail);
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
        self.transcript_collapsed +=
            super::trim_transcript(&mut self.transcript, super::MAX_TRANSCRIPT);
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
                // The AI just wrote this file -- refresh an open editor buffer so
                // it shows the new content (unless the user has unsaved edits).
                let abs = self.abs_path(&record.path);
                if let Some(buf) = self.open_files.get_mut(&abs)
                    && !buf.dirty
                    && let Ok(content) = self.fs.read_to_string(&abs)
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
