//! Every dashboard + chat interaction funnels through
//! [`RootView::dispatch`] — the single mutation choke point (the GPUI twin
//! of the egui branch's `ShellAction` queue). Split from `mod.rs` purely for
//! file size; this is the same `impl RootView`.

use std::path::PathBuf;

use gpui::{Context, Keystroke, Window};

use crate::session::{ComposerStyle, DashboardViewMode};
use crate::workspace::WorkspaceId;

use super::dashboard::{CardInput, InputKind};
use super::{RootView, Screen, chat, den};

/// Which open chat popover (agent / model switcher, composer style gear).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatPop {
    Agent(WorkspaceId),
    Model(WorkspaceId),
    Style,
}

/// Every dashboard interaction, funneled through [`RootView::dispatch`].
#[derive(Clone, Debug)]
pub enum DashAction {
    Pause(WorkspaceId),
    Resume(WorkspaceId),
    Stop(WorkspaceId),
    Restart(WorkspaceId),
    SetModel(WorkspaceId, String),
    /// Submit the open inline input (steer / new prompt).
    SubmitInput,
    CloseInput,
    /// Flip the open steer input's delivery mode (false = now, true = queue).
    SetSteerQueue(bool),
    TogglePopover(WorkspaceId),
    ClosePopover,
    /// Open a workspace's chat (cards, tabs, attention banner).
    Open(WorkspaceId),
    /// Open a workspace's chat focused on changes (diff chips live in the
    /// transcript; a dedicated diff view is still egui-branch-only).
    Changes(WorkspaceId),
    ShowDashboard,
    CloseWorkspace(WorkspaceId),
    SetView(DashboardViewMode),
    ToggleMotion,
    // -- chat --
    ChatSubmit(WorkspaceId),
    SetAgent(WorkspaceId, String),
    SetComposerStyle(ComposerStyle),
    ToggleChatPop(ChatPop),
    CloseChatPop,
    ApplyCompletion(WorkspaceId, usize),
    /// Toggle a transcript entry's collapsible body (diff / thinking).
    ToggleDiff(WorkspaceId, usize),
    ShowOlder(WorkspaceId),
    ToggleTree(WorkspaceId),
    ToggleDir(WorkspaceId, PathBuf),
    StarterPrompt(WorkspaceId, String),
    // -- needs-you answers --
    AskToggle(WorkspaceId, usize, usize),
    /// Open/close the free-text "Other" row for an ask question.
    AskOther(WorkspaceId, usize),
    AskSubmit(WorkspaceId),
    AskCancel(WorkspaceId),
    PendingChoose(WorkspaceId, usize),
    /// Send the answer input's text to a pending input prompt.
    PendingText(WorkspaceId),
    /// Enter pressed inside the shared answer input.
    AnswerEnter,
    /// Den interactions (join/leave/feed/kanban/plans/...).
    Den(den::DenAction),
}

impl RootView {
    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    /// The single mutation funnel for every dashboard interaction.
    pub fn dispatch(&mut self, action: DashAction, cx: &mut Context<Self>) {
        // Every dispatch is a user interaction (presence heuristic input).
        self.last_interaction = std::time::Instant::now();
        let accent = self.tokens.accent;
        let (run, paused_c, error_c) = (self.tokens.run, self.tokens.paused, self.tokens.error);
        match action {
            DashAction::Pause(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.pause_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} paused at next safe point"), paused_c);
                }
            }
            DashAction::Resume(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.resume_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} resumed"), run);
                }
            }
            DashAction::Stop(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.stop_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} stopped"), error_c);
                }
            }
            DashAction::Restart(id) => {
                let name = self.ws_name(id);
                self.supervisor.restart(id);
                self.toast(format!("Restarting {name}\u{2026}"), run);
            }
            DashAction::SetModel(id, model) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.set_model_live(&model);
                    let name = ws.name.clone();
                    self.toast(format!("{name} \u{2192} {model}"), accent);
                }
                self.model_popover = None;
            }
            DashAction::SubmitInput => self.submit_input(),
            DashAction::CloseInput => self.card_input = None,
            DashAction::SetSteerQueue(q) => {
                if let Some(input) = &mut self.card_input {
                    input.queue = q;
                }
            }
            DashAction::TogglePopover(id) => {
                self.model_popover = if self.model_popover == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }
            DashAction::ClosePopover => self.model_popover = None,
            DashAction::Open(id) => {
                self.ensure_chat_input(id, cx);
                self.screen = Screen::Chat(id);
                self.pending_focus = Some(id);
                self.chat_pop = None;
            }
            DashAction::Changes(id) => {
                self.ensure_chat_input(id, cx);
                self.screen = Screen::Chat(id);
                self.pending_focus = Some(id);
                self.toast(
                    "Changes are in the transcript chips \u{2014} a dedicated diff view is egui-only for now".to_string(),
                    accent,
                );
            }
            DashAction::ShowDashboard => {
                self.screen = Screen::Dashboard;
                self.chat_pop = None;
            }
            DashAction::CloseWorkspace(id) => {
                let name = self.ws_name(id);
                self.supervisor.close(id);
                self.chat_inputs.remove(&id);
                if self.screen == Screen::Chat(id) {
                    self.screen = Screen::Dashboard;
                }
                self.toast(format!("Closed {name}"), accent);
            }
            DashAction::ChatSubmit(id) => self.chat_submit(id, cx),
            DashAction::SetAgent(id, agent) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.set_agent_live(&agent);
                    let name = ws.name.clone();
                    self.toast(format!("{name} \u{2192} {agent}"), accent);
                }
            }
            DashAction::SetComposerStyle(style) => {
                self.composer_style = style;
                self.chat_pop = None;
                self.save_prefs();
                self.toast(format!("Composer: {}", style.label()), accent);
            }
            DashAction::ToggleChatPop(pop) => {
                self.chat_pop = if self.chat_pop.as_ref() == Some(&pop) {
                    None
                } else {
                    Some(pop)
                };
            }
            DashAction::CloseChatPop => self.chat_pop = None,
            DashAction::ApplyCompletion(id, index) => {
                let applied = self.supervisor.get(id).and_then(|ws| {
                    let item = ws.completion_items().get(index)?;
                    let input = self.chat_inputs.get(&id)?;
                    let text = input.read(cx).text().to_string();
                    Some((input.clone(), chat::composer::apply_completion(&text, item)))
                });
                if let Some((input, new_text)) = applied {
                    input.update(cx, |i, cx| i.set_text(new_text, cx));
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        ws.dismiss_completions();
                    }
                }
            }
            DashAction::ToggleDiff(id, idx) => {
                let key = (id.0, idx);
                if !self.expanded_entries.remove(&key) {
                    self.expanded_entries.insert(key);
                }
            }
            DashAction::ShowOlder(id) => {
                self.show_all_chat.insert(id);
            }
            DashAction::ToggleTree(id) => {
                if !self.tree_closed.remove(&id) {
                    self.tree_closed.insert(id);
                }
            }
            DashAction::ToggleDir(id, path) => {
                let key = (id.0, path);
                if !self.expanded_dirs.remove(&key) {
                    self.expanded_dirs.insert(key);
                }
            }
            DashAction::StarterPrompt(id, text) => {
                if let Some(input) = self.chat_inputs.get(&id) {
                    input.update(cx, |i, cx| i.set_text(text, cx));
                }
            }
            DashAction::AskToggle(id, qi, oi) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.ask_toggle_option(qi, oi);
                }
            }
            DashAction::AskOther(id, qi) => {
                self.ensure_answer_input(cx);
                self.other_target = if self.other_target == Some((id, qi)) {
                    None
                } else {
                    Some((id, qi))
                };
                if let Some(input) = &self.answer_input {
                    input.update(cx, |i, cx| i.clear(cx));
                }
            }
            DashAction::AskSubmit(id) => {
                // Flush the Other row into its question before submitting.
                if let Some((tid, qi)) = self.other_target.take()
                    && tid == id
                    && let Some(input) = &self.answer_input
                {
                    let text = input.read(cx).text().to_string();
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        ws.ask_set_other(qi, text);
                    }
                    input.update(cx, |i, cx| i.clear(cx));
                }
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.ask_submit();
                    let name = ws.name.clone();
                    self.toast(format!("Answered {name}"), accent);
                }
            }
            DashAction::AskCancel(id) => {
                self.other_target = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.ask_cancel();
                    let name = ws.name.clone();
                    self.toast(format!("Declined {name}'s question"), accent);
                }
            }
            DashAction::PendingChoose(id, i) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.pending_choose(i);
                    let name = ws.name.clone();
                    self.toast(format!("Answered {name}"), accent);
                }
            }
            DashAction::PendingText(id) => {
                let text = self
                    .answer_input
                    .as_ref()
                    .map(|i| i.read(cx).text().trim().to_string())
                    .unwrap_or_default();
                if text.is_empty() {
                    return;
                }
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.pending_answer_text(&text);
                    let name = ws.name.clone();
                    self.toast(format!("Answered {name}"), accent);
                }
                if let Some(input) = &self.answer_input {
                    input.update(cx, |i, cx| i.clear(cx));
                }
            }
            DashAction::Den(den_action) => {
                self.dispatch_den(den_action, cx);
                return;
            }
            DashAction::AnswerEnter => {
                // Route Enter in the answer input: input prompts submit
                // directly; ask "Other" rows wait for the explicit Submit.
                if let Screen::Chat(id) = self.screen {
                    let is_input_prompt = self
                        .supervisor
                        .get(id)
                        .and_then(|w| w.pending_request())
                        .is_some_and(|p| {
                            matches!(p.kind, crate::workspace::PendingKind::Input { .. })
                        });
                    if is_input_prompt {
                        self.dispatch(DashAction::PendingText(id), cx);
                    }
                }
            }
            DashAction::SetView(mode) => {
                self.dash_mode = mode;
                self.save_prefs();
            }
            DashAction::ToggleMotion => {
                self.reduce_motion = !self.reduce_motion;
                self.save_prefs();
                let state = if self.reduce_motion { "off" } else { "on" };
                self.toast(format!("Decorative motion {state}"), accent);
            }
        }
        cx.notify();
    }

    /// Toggle a card's inline input (one open card at a time) + focus it.
    pub fn toggle_input(
        &mut self,
        ws: WorkspaceId,
        kind: InputKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = matches!(&self.card_input, Some(i) if i.ws == ws && i.kind == kind);
        self.card_input = if open {
            None
        } else {
            window.focus(&self.input_focus);
            Some(CardInput {
                ws,
                kind,
                text: String::new(),
                queue: false,
            })
        };
        cx.notify();
    }

    /// Minimal inline-input editing: printable chars, backspace, cmd-V paste.
    /// (The full IME-aware text input arrives with the 2.3 composer.)
    pub fn edit_input(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let paste = ks.modifiers.platform && ks.key == "v";
        let clip = paste
            .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
            .flatten();
        let Some(input) = &mut self.card_input else {
            return;
        };
        if ks.key == "backspace" {
            input.text.pop();
        } else if let Some(text) = clip {
            input.text.push_str(&text);
        } else if !ks.modifiers.platform
            && !ks.modifiers.control
            && let Some(ch) = &ks.key_char
        {
            input.text.push_str(ch);
        } else {
            return;
        }
        cx.notify();
    }

    fn submit_input(&mut self) {
        let Some(input) = self.card_input.take() else {
            return;
        };
        let text = input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let accent = self.tokens.accent;
        let Some(ws) = self.supervisor.get_mut(input.ws) else {
            return;
        };
        let name = ws.name.clone();
        match input.kind {
            InputKind::Steer => {
                if ws.steer_text(&text, input.queue) {
                    let how = if input.queue {
                        "(queued \u{1f4e8})"
                    } else {
                        "now \u{1f3af}"
                    };
                    self.toast(format!("Steered {name} {how}"), accent);
                } else {
                    self.toast(
                        format!("{name} isn't running \u{2014} steer dropped"),
                        accent,
                    );
                }
            }
            InputKind::Send => {
                ws.send_prompt_text(&text);
                self.toast(format!("Sent {name}"), accent);
            }
        }
    }

    /// Composer submit: steer while a turn runs, otherwise send/dispatch.
    fn chat_submit(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        let Some(input) = self.chat_inputs.get(&id).cloned() else {
            return;
        };
        let text = input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let accent = self.tokens.accent;
        let Some(ws) = self.supervisor.get_mut(id) else {
            return;
        };
        let name = ws.name.clone();
        if ws.status == crate::workspace::InstanceStatus::Dead {
            self.toast(format!("{name} is stuck \u{2014} restart it first"), accent);
            return;
        }
        if ws.is_running_turn() {
            if ws.steer_text(&text, false) {
                self.toast(format!("Steered {name} now \u{1f3af}"), accent);
            }
        } else {
            ws.send_prompt_text(&text);
        }
        if let Some(ws) = self.supervisor.get_mut(id) {
            ws.dismiss_completions();
        }
        input.update(cx, |i, cx| i.clear(cx));
    }
}
