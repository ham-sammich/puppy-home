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

    pub(crate) fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.ready || self.running {
            return;
        }
        self.input.clear();
        self.record_history(&text);
        if text.starts_with('/') {
            self.dispatch_command(&text);
        } else if self.backend.is_some() {
            let images: Vec<String> = self
                .pending_images
                .iter()
                .map(|p| p.png_base64.clone())
                .collect();
            self.pending_images.clear();
            self.send_user_prompt(text, images);
        }
    }

    /// The one prompt-sending path (composer submit + dashboard "New prompt"):
    /// records the turn in the transcript and starts turn tracking. Signature
    /// converged with redesign/egui (text + images superset).
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
        // Optimistic: the card shows the prompt immediately; the next status
        // poll confirms it from the sidecar.
        self.last_prompt = text;
        self.begin_turn();
    }

    /// Card action: send a fresh prompt (or slash command) with no images.
    pub fn send_prompt_text(&mut self, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() || !self.ready || self.running || self.backend.is_none() {
            return;
        }
        if text.starts_with('/') {
            self.dispatch_command(&text);
        } else {
            self.send_user_prompt(text, Vec::new());
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
        let queue = self.steer_queue_mode;
        if self.steer_text(&text, queue) {
            self.input.clear();
        }
    }

    /// Switch the active agent live (sidecar re-announces via `Ready`).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn set_agent_live(&mut self, name: &str) {
        if let Some(backend) = &self.backend {
            backend.set_agent(name);
        }
    }

    /// Shell-style history recall, Up direction: the first recall stashes the
    /// in-progress draft, then each call walks one entry older. Mirrors the
    /// egui composer's ArrowUp branch exactly; `None` = nothing to recall.
    pub(crate) fn history_prev(&mut self, draft: &str) -> Option<String> {
        match self.history_pos {
            None => {
                if self.input_history.is_empty() {
                    return None;
                }
                self.history_stash = draft.to_string();
                self.history_pos = Some(self.input_history.len() - 1);
            }
            Some(0) => return None, // already at the oldest
            Some(p) => self.history_pos = Some(p - 1),
        }
        self.history_pos.map(|p| self.input_history[p].clone())
    }

    /// History recall, Down direction; walking past the newest entry restores
    /// the stashed draft (egui ArrowDown branch).
    pub(crate) fn history_next(&mut self) -> Option<String> {
        let p = self.history_pos?;
        if p + 1 < self.input_history.len() {
            self.history_pos = Some(p + 1);
            Some(self.input_history[p + 1].clone())
        } else {
            self.history_pos = None;
            Some(std::mem::take(&mut self.history_stash))
        }
    }

    /// Keep the completion engine from treating recalled text as fresh typing
    /// (egui parked `last_query` + hid the palette after a recall).
    pub(crate) fn suppress_completions_for(&mut self, text: &str) {
        self.last_query = text.to_string();
        self.comp_visible = false;
    }

    /// Card action: steer with explicit text + delivery mode. Returns whether
    /// it was actually sent (a finished turn can't be steered).
    pub fn steer_text(&mut self, text: &str, queue: bool) -> bool {
        if text.is_empty() || !self.running {
            return false;
        }
        let Some(backend) = &self.backend else {
            return false;
        };
        backend.steer(text, if queue { "queue" } else { "now" });
        let tag = if queue {
            "\u{1f4e8} steer (queued)"
        } else {
            "\u{1f3af} steer"
        };
        self.transcript.push(Entry::User(format!("{tag}: {text}")));
        if queue {
            // Optimistic; the next status poll reports the sidecar's count.
            self.queued_steers += 1;
        }
        true
    }

    /// Card action: pause the running turn at the next safe boundary.
    pub fn pause_turn(&mut self) {
        if self.running
            && !self.paused
            && let Some(backend) = &self.backend
        {
            backend.pause();
            self.paused = true;
        }
    }

    /// Card action: resume a turn held at the pause gate.
    pub fn resume_turn(&mut self) {
        if self.running
            && self.paused
            && let Some(backend) = &self.backend
        {
            backend.resume();
            self.paused = false;
        }
    }

    /// Card action: cancel the running turn.
    pub fn stop_turn(&mut self) {
        if self.running
            && let Some(backend) = &self.backend
        {
            backend.cancel();
        }
    }

    /// Card action: switch the active model live (sidecar re-announces).
    pub fn set_model_live(&mut self, name: &str) {
        if let Some(backend) = &self.backend {
            backend.set_model(name);
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
