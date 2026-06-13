//! Every dashboard + chat interaction funnels through
//! [`RootView::dispatch`] — the single mutation choke point (the GPUI twin
//! of the egui branch's `ShellAction` queue). Split from `mod.rs` purely for
//! file size; this is the same `impl RootView`.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{Context, Keystroke, Pixels, Point, Window};

use crate::session::{ComposerStyle, DashboardViewMode};
use crate::workspace::{InstanceStatus, WorkspaceId};

use super::dashboard::{CardInput, InputKind};
use super::{RootView, Screen, chat, den};

/// Which open chat popover (agent / model switcher, composer style gear,
/// the @File picker browsing some directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatPop {
    Agent(WorkspaceId),
    Model(WorkspaceId),
    Style,
    FilePicker(WorkspaceId, PathBuf),
    /// Tree-row context panel: (workspace, path, is_dir).
    TreeMenu(WorkspaceId, PathBuf, bool),
    /// Composer context/history-commands pill (next to the ctx chip).
    Context(WorkspaceId),
    /// Composer general slash-command menu (left of the input box).
    Commands(WorkspaceId),
    /// Composer ctx chip popover: the full /context breakdown (#2).
    CtxInfo(WorkspaceId),
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
    /// Spawn a new instance at the user's home directory — the dashboard
    /// whistle and the toolbar "New Chat" (`to_chat` jumps straight into
    /// the new workspace's chat).
    OpenHome {
        to_chat: bool,
    },
    /// About / version panel (toolbar chip).
    About(crate::gpui_ui::about::AboutAction),
    /// Avatar picker (toolbar identity chip, QW8).
    Avatar(crate::gpui_ui::avatars::AvatarAction),
    /// Open a workspace's chat focused on changes (diff chips live in the
    /// transcript; a dedicated diff view is still egui-branch-only).
    Changes(WorkspaceId),
    ShowDashboard,
    CloseWorkspace(WorkspaceId),
    /// Drag-reorder (#5): move `moved` to sit just before `target` in the
    /// user-facing order (tabs + dashboard cards + persisted session).
    ReorderWorkspace {
        moved: WorkspaceId,
        target: WorkspaceId,
    },
    /// Dashboard close request: resting/dead closes now; a busy puppy arms a
    /// confirm first (so we never silently kill a running process).
    RequestCloseWorkspace(WorkspaceId),
    /// Abandon a pending dashboard close confirmation.
    CancelCloseWorkspace,
    SetView(DashboardViewMode),
    ToggleMotion,
    // -- chat --
    ChatSubmit(WorkspaceId),
    SetAgent(WorkspaceId, String),
    SetComposerStyle(ComposerStyle),
    ToggleChatPop(ChatPop),
    /// Send a slash command exactly as if typed in the composer (QW9).
    SendCommand(WorkspaceId, String),
    /// Seed the composer input (parametrized commands like /truncate N).
    SeedCommand(WorkspaceId, String),
    CloseChatPop,
    ApplyCompletion(WorkspaceId, usize),
    /// Toggle a transcript entry's collapsible body (diff / thinking).
    ToggleDiff(WorkspaceId, usize),
    ShowOlder(WorkspaceId),
    ToggleTree(WorkspaceId),
    /// Cycle the explorer's hidden-entry policy (Show -> Dim -> Hide, F4).
    CycleHidden,
    /// Open the floating tree context menu at a window-space cursor point.
    OpenTreeMenu(WorkspaceId, PathBuf, bool, Point<Pixels>),
    /// Copy a tree entry's absolute (false) or workspace-relative (true) path.
    TreeCopyPath(WorkspaceId, PathBuf, bool),
    /// Reveal a tree entry in the OS file manager (is_dir picks select-vs-open).
    TreeReveal(PathBuf, bool),
    /// Copy arbitrary text to the clipboard (chat message / code block copy).
    CopyText(String),
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
    /// Steer delivery mode for composer-dock steering (false = now).
    SetChatSteerQueue(bool),
    /// Open the @File picker at the workspace root.
    PickerOpen(WorkspaceId),
    /// Navigate the @File picker into a directory.
    PickerDir(WorkspaceId, PathBuf),
    /// Jump the @File picker to the path typed in its path bar (F6).
    PickerGoPath,
    /// Insert an `@<path>` reference for the picked file.
    PickerPick(WorkspaceId, PathBuf),
    /// Drop a pending pasted image before sending.
    RemoveImage(WorkspaceId, usize),
    /// `+ New chat`: /clear machinery (transcript wipe + fresh session).
    NewChat(WorkspaceId),
    /// Workspace-toolbar "push creds" for a REMOTE workspace: push local
    /// auth + model config to its host (two-step armed confirm).
    PushCreds(WorkspaceId),
    ToggleLogs(WorkspaceId),
    OpenSessions(WorkspaceId),
    CloseSessions,
    SessionsRefresh(WorkspaceId),
    /// Select a session in the browser `(name, source)` -> preview it.
    SessionSelect(WorkspaceId, String, String),
    SessionResume(WorkspaceId, String, String),
    /// Toggle a thinking fold (auto-collapse one-shot still wins first).
    ToggleThinking(WorkspaceId, usize),
    // -- editor area --
    OpenEditorFile(WorkspaceId, PathBuf),
    EditorTab(WorkspaceId, usize),
    /// Close a tab (dirty files ask once: second click within the confirm
    /// state closes for real).
    EditorClose(WorkspaceId, usize),
    EditorSave(WorkspaceId, PathBuf),
    /// Load a git working-tree change into the Changes tab.
    LoadGitChange(WorkspaceId, String, char),
    /// Load a Code-Puppy-reported diff (non-git fallback).
    LoadDiffIndex(WorkspaceId, usize),
    // -- tree ops --
    TreeRename(WorkspaceId, PathBuf),
    TreeNew(WorkspaceId, PathBuf, bool),
    TreeOpSubmit,
    /// Abandon an armed rename/new op without performing it (F5).
    TreeOpCancel,
    TreeDelete(WorkspaceId, PathBuf, bool),
    TreeDeleteConfirm,
    // -- git surface --
    ShowGit(WorkspaceId),
    GitRefresh(WorkspaceId),
    GitFetch(WorkspaceId),
    GitPull(WorkspaceId),
    GitPush(WorkspaceId),
    GitStage(WorkspaceId, String),
    GitUnstage(WorkspaceId, String),
    GitStageAll(WorkspaceId),
    GitUnstageAll(WorkspaceId),
    GitCommit(WorkspaceId),
    GitToggleGraph(WorkspaceId),
    /// Open a commit's patch: (index, from-flat-list?).
    GitOpenCommit(WorkspaceId, usize, bool),
    GraphMenu(WorkspaceId, (String, String, Vec<String>)),
    GraphMenuClose,
    GraphCheckout(WorkspaceId, String),
    GraphMerge(WorkspaceId, String),
    GraphNewBranch(WorkspaceId, String),
    GraphBranchSubmit,
    GraphCherryPick(WorkspaceId, String),
    GraphRevert(WorkspaceId, String),
    GraphReset(WorkspaceId, String),
    CredsSubmit(WorkspaceId),
    CredsCancel(WorkspaceId),
    BlameToggle(WorkspaceId, PathBuf),
    // -- terminal --
    TermToggle(WorkspaceId),
    TermInput(WorkspaceId, Vec<u8>),
    TermScroll(WorkspaceId, i32),
    /// Den interactions (join/leave/feed/kanban/plans/...).
    Den(den::DenAction),
    /// Manager overlays (MCP / Skills / Agents).
    Mgr(super::managers::MgrAction),
    /// The remote SSH connect dialog.
    Remote(super::remote::RemoteAction),
    /// Theme picker + editor.
    Theme(super::theme_ui::ThemeAction),
    /// The browser-plugin host surface (+ dashboard plugins section).
    Browser(super::browser_ui::BrowserAction),
    /// The performance HUD (toolbar fleet-stats click).
    PerfToggle,
}

/// What the tree-op input is editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeOp {
    Rename(WorkspaceId, PathBuf),
    New(WorkspaceId, PathBuf, bool),
}

/// `@<path>` reference token, exactly like the egui composer: paths under
/// the workspace root go relative with forward slashes; outside stays
/// absolute.
fn file_token(root: &std::path::Path, path: &std::path::Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    format!("@{}", rel.to_string_lossy().replace('\\', "/"))
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
            DashAction::OpenHome { to_chat } => {
                let home = std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from);
                match home {
                    Some(home) => match self.supervisor.open(home) {
                        Ok(id) => {
                            self.last_error = None;
                            if to_chat {
                                self.ensure_chat_input(id, cx);
                                self.screen = Screen::Chat(id);
                                self.pending_focus = Some(id);
                            }
                        }
                        Err(e) => self.last_error = Some(e),
                    },
                    None => self.last_error = Some("No home directory found".into()),
                }
            }
            DashAction::Changes(id) => {
                self.ensure_chat_input(id, cx);
                self.screen = Screen::Chat(id);
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.show_changes();
                }
            }
            DashAction::ShowDashboard => {
                self.screen = Screen::Dashboard;
                self.chat_pop = None;
            }
            DashAction::RequestCloseWorkspace(id) => {
                // Resting (Idle) or already-dead = no live process to kill, so
                // close immediately; anything else asks first.
                let safe = self
                    .supervisor
                    .get(id)
                    .map(|w| matches!(w.status, InstanceStatus::Idle | InstanceStatus::Dead))
                    .unwrap_or(true);
                if safe {
                    self.dispatch(DashAction::CloseWorkspace(id), cx);
                } else {
                    self.card_close_confirm = Some(id);
                }
            }
            DashAction::ReorderWorkspace { moved, target } => {
                self.supervisor.reorder(moved, target);
            }
            DashAction::CancelCloseWorkspace => self.card_close_confirm = None,
            DashAction::CloseWorkspace(id) => {
                let name = self.ws_name(id);
                self.card_close_confirm = None;
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
            DashAction::SendCommand(id, cmd) => {
                // The exact typed path: send_prompt_text routes '/' through
                // dispatch_command and no-ops mid-turn, same as typing.
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.send_prompt_text(&cmd);
                }
                self.chat_pop = None;
            }
            DashAction::SeedCommand(id, cmd) => {
                self.ensure_chat_input(id, cx);
                if let Some(input) = self.chat_inputs.get(&id) {
                    input.update(cx, |i, cx| i.set_text(cmd, cx));
                }
                self.pending_focus = Some(id);
                self.chat_pop = None;
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
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        // Inserted text must not immediately re-open the
                        // palette as "fresh typing".
                        ws.suppress_completions_for(&new_text);
                        ws.dismiss_completions();
                    }
                    input.update(cx, |i, cx| i.set_text(new_text, cx));
                    self.palette_sel = 0;
                    self.sync_palette_flag(id, cx);
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
            DashAction::CycleHidden => {
                self.hidden_mode = self.hidden_mode.next();
            }
            DashAction::OpenTreeMenu(id, path, is_dir, pos) => {
                self.chat_pop = Some(ChatPop::TreeMenu(id, path, is_dir));
                self.tree_menu_pos = Some(pos);
                self.tree_op = None;
                self.tree_delete_confirm = None;
            }
            DashAction::TreeCopyPath(id, path, relative) => {
                let text = if relative {
                    self.supervisor
                        .get(id)
                        .and_then(|ws| path.strip_prefix(&ws.root).ok())
                        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| path.to_string_lossy().into_owned())
                } else {
                    path.to_string_lossy().into_owned()
                };
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                self.toast(format!("\u{1f4cb} {text}"), accent);
                self.chat_pop = None;
            }
            DashAction::CopyText(text) => {
                let n = text.chars().count();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                self.toast(format!("\u{1f4cb} Copied {n} chars"), accent);
            }
            DashAction::TreeReveal(path, is_dir) => {
                crate::proc::reveal_in_file_manager(&path, is_dir);
                self.chat_pop = None;
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
            DashAction::Mgr(mgr_action) => {
                self.dispatch_mgr(mgr_action, cx);
                return;
            }
            DashAction::Remote(remote_action) => {
                self.dispatch_remote(remote_action, cx);
                return;
            }
            DashAction::Theme(theme_action) => {
                self.dispatch_theme(theme_action, cx);
                return;
            }
            DashAction::Browser(browser_action) => {
                self.dispatch_browser(browser_action, cx);
                return;
            }
            DashAction::PerfToggle => {
                self.perf.visible = !self.perf.visible;
                cx.notify();
                return;
            }
            DashAction::SetChatSteerQueue(q) => self.chat_steer_queue = q,
            DashAction::PickerOpen(id) => {
                let root_dir = self.supervisor.get(id).map(|w| w.root.clone());
                if let Some(dir) = root_dir {
                    self.seed_picker_path(&dir, cx);
                    self.chat_pop = Some(ChatPop::FilePicker(id, dir));
                }
            }
            DashAction::PickerDir(id, dir) => {
                self.seed_picker_path(&dir, cx);
                self.chat_pop = Some(ChatPop::FilePicker(id, dir));
            }
            DashAction::PickerGoPath => {
                // Jump to the path typed in the picker's path bar (F6): any
                // folder or drive, not just dirs under the workspace root.
                if let Some(ChatPop::FilePicker(id, _)) = self.chat_pop.clone() {
                    let text = self
                        .picker_path_input
                        .as_ref()
                        .map(|i| i.read(cx).text().trim().to_string())
                        .unwrap_or_default();
                    if !text.is_empty() {
                        self.chat_pop =
                            Some(ChatPop::FilePicker(id, std::path::PathBuf::from(text)));
                    }
                }
            }
            DashAction::PickerPick(id, path) => {
                self.chat_pop = None;
                let token = self
                    .supervisor
                    .get(id)
                    .map(|ws| file_token(&ws.root, &path))
                    .unwrap_or_default();
                if let Some(input) = self.chat_inputs.get(&id).cloned() {
                    let mut text = input.read(cx).text().to_string();
                    if !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                    }
                    text.push_str(&token);
                    text.push(' ');
                    input.update(cx, |i, cx| i.set_text(text, cx));
                }
            }
            DashAction::RemoveImage(id, idx) => {
                if let Some(imgs) = self.pending_images.get_mut(&id)
                    && idx < imgs.len()
                {
                    imgs.remove(idx);
                }
            }
            DashAction::NewChat(id) => {
                if let Some(ws) = self.supervisor.get_mut(id)
                    && ws.new_chat()
                {
                    let name = ws.name.clone();
                    // Fresh chat: stale per-entry UI state dies with it.
                    self.expanded_entries.retain(|(wid, _)| *wid != id.0);
                    self.collapsed_thinking.retain(|(wid, _)| *wid != id.0);
                    self.show_all_chat.remove(&id);
                    self.toast(format!("New chat in {name}"), accent);
                }
            }
            DashAction::ToggleLogs(id) => {
                if !self.logs_open.remove(&id) {
                    self.logs_open.insert(id);
                }
            }
            DashAction::PushCreds(id) => {
                if self.creds_pending.is_some() {
                    return; // one push at a time
                }
                // Two-step: first click arms, second sends (credentials).
                if self.creds_confirm != Some(id) {
                    self.creds_confirm = Some(id);
                    return;
                }
                self.creds_confirm = None;
                let Some(ws) = self.supervisor.get(id) else {
                    return;
                };
                let Some(target) = ws.remote_target() else {
                    return; // local workspace: button isn't rendered anyway
                };
                self.creds_pending = Some(crate::gpui_ui::remote::CredsPush {
                    label: target.destination(),
                    ws: Some(id),
                    rx: crate::gpui_ui::remote::spawn_push(self.waker.clone(), target),
                });
            }
            DashAction::OpenSessions(id) => {
                self.ensure_sessions_filter_input(cx);
                self.sessions_open = Some(id);
                self.session_selected = None;
                if let Some(ws) = self.supervisor.get(id) {
                    ws.request_sessions();
                }
            }
            DashAction::CloseSessions => {
                self.sessions_open = None;
                self.session_selected = None;
            }
            DashAction::SessionsRefresh(id) => {
                if let Some(ws) = self.supervisor.get(id) {
                    ws.request_sessions();
                }
            }
            DashAction::SessionSelect(id, name, source) => {
                let fresh = self
                    .session_selected
                    .as_ref()
                    .map(|(n, _)| n != &name)
                    .unwrap_or(true);
                if fresh && let Some(ws) = self.supervisor.get_mut(id) {
                    ws.request_session_preview(&name, &source);
                }
                self.session_selected = Some((name, source));
            }
            DashAction::SessionResume(id, name, source) => {
                let resumed = self
                    .supervisor
                    .get_mut(id)
                    .map(|ws| ws.resume_session(&name, &source))
                    .unwrap_or(false);
                if resumed {
                    self.sessions_open = None;
                    self.session_selected = None;
                    self.expanded_entries.retain(|(wid, _)| *wid != id.0);
                    self.collapsed_thinking.retain(|(wid, _)| *wid != id.0);
                    self.toast(
                        format!("Resuming {}", crate::workspace::short_session(&name)),
                        accent,
                    );
                } else {
                    self.toast(
                        "Stop the running turn before loading a session".into(),
                        accent,
                    );
                }
            }
            DashAction::ToggleThinking(id, idx) => {
                let key = (id.0, idx);
                if !self.collapsed_thinking.remove(&key) {
                    self.collapsed_thinking.insert(key);
                }
            }
            DashAction::OpenEditorFile(id, path) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.open_editor_file(path.clone());
                }
                self.ensure_editor_input(id, &path, cx);
                self.chat_pop = None;
            }
            DashAction::EditorTab(id, ix) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.set_editor_active(ix);
                }
                self.editor_close_confirm = None;
            }
            DashAction::EditorClose(id, ix) => {
                let dirty = self
                    .supervisor
                    .get(id)
                    .map(|ws| match ws.editor_tabs().get(ix) {
                        Some(crate::workspace::EditorItem::File(p)) => ws.is_file_dirty(p),
                        _ => false,
                    })
                    .unwrap_or(false);
                if dirty && self.editor_close_confirm != Some((id, ix)) {
                    self.editor_close_confirm = Some((id, ix));
                } else {
                    self.editor_close_confirm = None;
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        ws.close_editor(ix);
                    }
                }
            }
            DashAction::EditorSave(id, path) => {
                let saved = self
                    .supervisor
                    .get_mut(id)
                    .map(|ws| ws.save_file(&path))
                    .unwrap_or(false);
                if saved {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.toast(format!("Saved {name}"), accent);
                }
            }
            DashAction::LoadGitChange(id, path, marker) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.load_git_diff(&path, marker);
                }
            }
            DashAction::LoadDiffIndex(id, ix) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.load_diff_index(ix);
                }
            }
            DashAction::TreeRename(id, path) => {
                self.ensure_tree_op_input(cx);
                let seed = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Some(input) = &self.tree_op_input {
                    input.update(cx, |i, cx| i.set_text(seed, cx));
                }
                self.tree_op = Some(TreeOp::Rename(id, path));
                self.tree_delete_confirm = None;
                self.pending_tree_focus = true;
            }
            DashAction::TreeNew(id, parent, is_dir) => {
                self.ensure_tree_op_input(cx);
                if let Some(input) = &self.tree_op_input {
                    input.update(cx, |i, cx| i.clear(cx));
                }
                self.tree_op = Some(TreeOp::New(id, parent, is_dir));
                self.tree_delete_confirm = None;
                self.pending_tree_focus = true;
            }
            DashAction::TreeOpSubmit => {
                let Some(op) = self.tree_op.take() else {
                    return;
                };
                let name = self
                    .tree_op_input
                    .as_ref()
                    .map(|i| i.read(cx).text().trim().to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    return;
                }
                let result = match &op {
                    TreeOp::Rename(id, path) => self
                        .supervisor
                        .get_mut(*id)
                        .map(|ws| ws.perform_rename(path, &name)),
                    TreeOp::New(id, parent, is_dir) => self
                        .supervisor
                        .get_mut(*id)
                        .map(|ws| ws.perform_new(parent, *is_dir, &name)),
                };
                match result {
                    Some(Ok(())) => self.toast(format!("\u{2713} {name}"), accent),
                    Some(Err(e)) => self.toast(e, self.tokens.error),
                    None => {}
                }
                self.chat_pop = None;
            }
            DashAction::TreeOpCancel => {
                self.tree_op = None;
                self.chat_pop = None;
            }
            DashAction::TreeDelete(id, path, is_dir) => {
                self.tree_delete_confirm = Some((id, path, is_dir));
            }
            DashAction::TreeDeleteConfirm => {
                let Some((id, path, is_dir)) = self.tree_delete_confirm.take() else {
                    return;
                };
                let result = self
                    .supervisor
                    .get_mut(id)
                    .map(|ws| ws.delete_path(&path, is_dir));
                match result {
                    Some(Ok(())) => self.toast("Deleted".to_string(), accent),
                    Some(Err(e)) => self.toast(e, self.tokens.error),
                    None => {}
                }
                self.chat_pop = None;
            }
            DashAction::ShowGit(id) => {
                self.ensure_chat_input(id, cx);
                self.ensure_commit_input(id, cx);
                self.screen = Screen::Chat(id);
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.show_git();
                }
            }
            DashAction::GitRefresh(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.refresh_git_view();
                }
            }
            DashAction::GitFetch(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_fetch();
                }
            }
            DashAction::GitPull(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_pull();
                }
            }
            DashAction::GitPush(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_push();
                }
            }
            DashAction::GitStage(id, path) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_stage_path(&path);
                }
            }
            DashAction::GitUnstage(id, path) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_unstage_path(&path);
                }
            }
            DashAction::GitStageAll(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_stage_all();
                }
            }
            DashAction::GitUnstageAll(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_unstage_all();
                }
            }
            DashAction::GitCommit(id) => {
                let msg = self
                    .commit_inputs
                    .get(&id)
                    .map(|i| i.read(cx).text().trim().to_string())
                    .unwrap_or_default();
                let staged = self
                    .supervisor
                    .get(id)
                    .and_then(|w| w.git_view_data().map(|v| !v.staged.is_empty()))
                    .unwrap_or(false);
                if msg.is_empty() || !staged {
                    self.toast("Stage something and write a message first".into(), accent);
                    return;
                }
                let ok = self
                    .supervisor
                    .get_mut(id)
                    .map(|ws| ws.git_commit_msg(&msg))
                    .unwrap_or(false);
                if ok && let Some(input) = self.commit_inputs.get(&id) {
                    input.update(cx, |i, cx| i.clear(cx));
                }
            }
            DashAction::GitToggleGraph(id) => {
                if !self.git_list_mode.remove(&id) {
                    self.git_list_mode.insert(id);
                }
            }
            DashAction::GitOpenCommit(id, ix, from_list) => {
                let commit = self.supervisor.get(id).and_then(|ws| {
                    if from_list {
                        ws.git_view_data().and_then(|v| v.log.get(ix).cloned())
                    } else {
                        ws.graph_commits().get(ix).cloned()
                    }
                });
                if let (Some(c), Some(ws)) = (commit, self.supervisor.get_mut(id)) {
                    ws.open_commit(&c);
                }
            }
            DashAction::GraphMenu(id, target) => {
                let _ = id;
                self.graph_menu = Some(target);
            }
            DashAction::GraphMenuClose => self.graph_menu = None,
            DashAction::GraphCheckout(id, name) => {
                self.graph_menu = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_checkout(&name);
                }
            }
            DashAction::GraphMerge(id, name) => {
                self.graph_menu = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_merge(&name);
                }
            }
            DashAction::GraphNewBranch(id, at) => {
                self.ensure_branch_input(cx);
                self.branch_target = Some((id, at));
                self.graph_menu = None;
            }
            DashAction::GraphBranchSubmit => {
                let Some((id, at)) = self.branch_target.take() else {
                    return;
                };
                let name = self
                    .branch_input
                    .as_ref()
                    .map(|i| i.read(cx).text().trim().to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    return;
                }
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_create_branch(&name, &at);
                }
                if let Some(input) = &self.branch_input {
                    input.update(cx, |i, cx| i.clear(cx));
                }
            }
            DashAction::GraphCherryPick(id, hash) => {
                self.graph_menu = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_cherry_pick(&hash);
                }
            }
            DashAction::GraphRevert(id, hash) => {
                self.graph_menu = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_revert(&hash);
                }
            }
            DashAction::GraphReset(id, hash) => {
                self.graph_menu = None;
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_reset(&hash, "--hard");
                }
            }
            DashAction::CredsSubmit(id) => {
                let user = self
                    .creds_user_input
                    .as_ref()
                    .map(|i| i.read(cx).text().to_string())
                    .unwrap_or_default();
                let pass = self
                    .creds_pass_input
                    .as_ref()
                    .map(|i| i.read(cx).text().to_string())
                    .unwrap_or_default();
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_creds_submit(user, pass);
                }
                // Clear the password either way; keep the username for retry.
                if let Some(input) = &self.creds_pass_input {
                    input.update(cx, |i, cx| i.clear(cx));
                }
            }
            DashAction::CredsCancel(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.git_creds_cancel();
                }
            }
            DashAction::BlameToggle(id, path) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.toggle_blame(&path);
                }
            }
            DashAction::TermToggle(id) => {
                let on = self
                    .supervisor
                    .get(id)
                    .map(|w| w.terminal_visible())
                    .unwrap_or(false);
                let waker = self.waker.clone();
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.set_terminal_visible(!on, &waker);
                }
            }
            DashAction::TermInput(id, bytes) => {
                if let Some(term) = self.supervisor.get_mut(id).and_then(|ws| ws.terminal_mut()) {
                    term.send_bytes(&bytes);
                }
            }
            DashAction::TermScroll(id, delta) => {
                if let Some(term) = self.supervisor.get_mut(id).and_then(|ws| ws.terminal_mut()) {
                    term.scroll_lines(delta);
                }
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
            DashAction::About(a) => {
                use crate::gpui_ui::about::AboutAction;
                match a {
                    AboutAction::Toggle => self.about.open = !self.about.open,
                    AboutAction::Check => self.about.check(self.waker.clone()),
                    AboutAction::Update => self.about.update(self.waker.clone()),
                }
            }
            DashAction::Avatar(a) => {
                use crate::gpui_ui::avatars::{AvatarAction, AvatarKind};
                match a {
                    AvatarAction::Toggle => {
                        self.avatar_ui.open = !self.avatar_ui.open;
                        if self.avatar_ui.open && self.avatar_input.is_none() {
                            let entity = cx.new(|cx| {
                                crate::gpui_ui::input::ChatInput::new(
                                    "paste any other emoji\u{2026}",
                                    cx,
                                )
                            });
                            let sub = cx
                                .subscribe(&entity, |_, _, _: &crate::gpui_ui::InputEvent, cx| {
                                    cx.notify()
                                });
                            self.avatar_input = Some(entity);
                            self.chat_subs.push(sub);
                        }
                    }
                    AvatarAction::Target(kind) => self.avatar_ui.target = kind,
                    AvatarAction::Pick(emoji) => {
                        match self.avatar_ui.target {
                            AvatarKind::User => self.user_avatar = emoji,
                            AvatarKind::Puppy => self.puppy_avatar = emoji,
                        }
                        self.save_prefs();
                    }
                    AvatarAction::ApplyCustom => {
                        let text = self
                            .avatar_input
                            .as_ref()
                            .map(|i| i.read(cx).text().trim().to_string())
                            .unwrap_or_default();
                        if !text.is_empty() {
                            match self.avatar_ui.target {
                                AvatarKind::User => self.user_avatar = text,
                                AvatarKind::Puppy => self.puppy_avatar = text,
                            }
                            self.save_prefs();
                        }
                    }
                    AvatarAction::PickPhoto => {
                        // gpui's path prompt can't filter extensions, so use
                        // rfd with an image-only filter. rfd must run OFF the
                        // render loop (its own native modal pump) — same
                        // worker-thread pattern the egui branch uses (F11).
                        let kind = self.avatar_ui.target;
                        let (tx, rx) =
                            futures::channel::oneshot::channel::<Option<std::path::PathBuf>>();
                        std::thread::spawn(move || {
                            let picked = rfd::FileDialog::new()
                                .set_title("Choose a profile picture")
                                .add_filter(
                                    "Images",
                                    &[
                                        "png", "jpg", "jpeg", "jfif", "webp", "gif", "bmp",
                                        "tif", "tiff", "ico",
                                    ],
                                )
                                .pick_file();
                            let _ = tx.send(picked);
                        });
                        cx.spawn(async move |this, cx| {
                            if let Ok(Some(src)) = rx.await {
                                let stored =
                                    crate::gpui_ui::avatars::store_photo(&src, kind);
                                let _ = this.update(cx, |root, cx| {
                                    match stored {
                                        Some(path) => {
                                            match kind {
                                                AvatarKind::User => root.user_avatar = path,
                                                AvatarKind::Puppy => root.puppy_avatar = path,
                                            }
                                            root.save_prefs();
                                        }
                                        None => {
                                            root.last_error =
                                                Some("Couldn't import that image".into())
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                        })
                        .detach();
                    }
                }
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
        } else if ks.key == "space" {
            // gpui leaves `key_char` empty for the space key on some
            // platforms, so the printable branch below would drop it —
            // handle it explicitly (the spacebar bug).
            input.text.push(' ');
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
            // Mid-turn Enter = steer, honoring the dock's now/queue toggle.
            // Pending images stay attached for the next real prompt.
            let queue = self.chat_steer_queue;
            if ws.steer_text(&text, queue) {
                let how = if queue {
                    "(queued \u{1f4e8})"
                } else {
                    "now \u{1f3af}"
                };
                self.toast(format!("Steered {name} {how}"), accent);
            }
        } else if text.starts_with('/') {
            ws.send_prompt_text(&text); // slash command; images stay pending
        } else {
            if !ws.is_ready() {
                self.toast(format!("{name} isn't ready yet"), accent);
                return;
            }
            let images: Vec<String> = self
                .pending_images
                .remove(&id)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.b64)
                .collect();
            if let Some(ws) = self.supervisor.get_mut(id) {
                ws.send_user_prompt(text, images);
            }
        }
        if let Some(ws) = self.supervisor.get_mut(id) {
            ws.dismiss_completions();
        }
        self.palette_sel = 0;
        self.sync_palette_flag(id, cx);
        input.update(cx, |i, cx| i.clear(cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_token_relativizes_under_root() {
        let root = std::path::Path::new("/repo");
        assert_eq!(
            file_token(root, std::path::Path::new("/repo/src/main.rs")),
            "@src/main.rs"
        );
        assert_eq!(
            file_token(root, std::path::Path::new("/elsewhere/x.txt")),
            "@/elsewhere/x.txt"
        );
    }
}
