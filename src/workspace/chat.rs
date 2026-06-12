//! Turn lifecycle + command dispatch: send/steer/pause/answer.

use std::time::Instant;

use super::Workspace;
use super::state::{Entry, InstanceStatus, PendingKind};

impl Workspace {
    /// Reset per-turn tracking and mark the workspace as running.
    pub(crate) fn begin_turn(&mut self) {
        self.tool_calls = 0;
        self.current_tool = None;
        self.sub_agents.clear();
        self.paused = false;
        self.running = true;
        self.turn_started = Some(Instant::now());
        self.status_req_at = Instant::now();
        self.set_status(InstanceStatus::Running);
    }

    /// Dispatch a slash command (shared by the composer + the Commands menu).
    /// `/clear` also wipes the on-screen transcript to mirror the agent reset.
    pub(crate) fn dispatch_command(&mut self, text: &str) {
        let Some(backend) = &self.backend else { return };
        if !self.ready || self.running {
            return;
        }
        backend.send_command(text);
        let first = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if first == "/clear" || first == "/cls" {
            self.transcript.clear();
            self.transcript_collapsed = 0;
            self.transcript.push(Entry::Note(format!(
                "🧹 Conversation cleared — {} starts fresh.",
                self.puppy_name
            )));
        } else {
            self.transcript.push(Entry::User(text.to_string()));
        }
        self.begin_turn();
    }

    /// Remember a line sent from the composer for Up/Down recall (skips
    /// consecutive duplicates, shell-style).
    fn record_history(&mut self, text: &str) {
        if self.input_history.last().map(String::as_str) != Some(text) {
            self.input_history.push(text.to_string());
        }
        self.history_pos = None;
        self.history_stash.clear();
    }

    /// The one prompt-sending path (composer submit + dashboard "New prompt"):
    /// records the turn in the transcript and starts turn tracking. Signature
    /// converged across BOTH UI branches (text + images superset) — keep all
    /// three trees identical here or cherry-picks will trip (again).
    pub(crate) fn send_user_prompt(&mut self, text: String, images: Vec<String>) {
        let Some(backend) = &self.backend else { return };
        self.transcript.push(Entry::User(text.clone()));
        if !images.is_empty() {
            self.transcript.push(Entry::Note(format!(
                "Sent {} image(s) with this message.",
                images.len()
            )));
        }
        backend.send_prompt(&text, &images);
        // Optimistic: cards show the prompt immediately; the next status poll
        // confirms it from the sidecar.
        self.last_prompt = text;
        self.begin_turn();
    }

    pub(crate) fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.ready || self.running {
            return;
        }
        self.input.clear();
        self.record_history(&text);
        if text.starts_with('/') {
            self.dispatch_command(&text);
        } else {
            let images: Vec<String> = self
                .pending_images
                .iter()
                .map(|p| p.png_base64.clone())
                .collect();
            self.pending_images.clear();
            self.send_user_prompt(text, images);
        }
    }

    /// Inject a steering message into the running turn (now = mid-turn, queue =
    /// after this turn). Clears the input box.
    pub(crate) fn steer(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.running {
            return;
        }
        self.record_history(&text);
        let mode = if self.steer_queue_mode {
            "queue"
        } else {
            "now"
        };
        if let Some(backend) = &self.backend {
            backend.steer(&text, mode);
            self.input.clear();
            let tag = if self.steer_queue_mode {
                "📨 steer (queued)"
            } else {
                "🎯 steer"
            };
            self.transcript.push(Entry::User(format!("{tag}: {text}")));
        }
    }

    /// Toggle pause/resume of the running turn (optimistic; confirmed by event).
    pub(crate) fn toggle_pause(&mut self) {
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

    pub(crate) fn answer_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let Some(backend) = &self.backend else { return };
        match &pending.kind {
            PendingKind::Input { .. } => {
                backend.respond_input(&pending.prompt_id, &pending.text);
                self.transcript
                    .push(Entry::User(format!("↳ {}", pending.text)));
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
