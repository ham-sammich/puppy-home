//! "Connect to a remote folder" — egui `views/remote_connect.rs` +
//! `app/remote.rs` at parity, dressed in the GPUI tokens.
//!
//! The dialog: pick a host from `~/.ssh/config` (or type
//! `[user@]host[:port]`), choose a remote folder — typed or via the
//! built-in folder browser (SSH `ls`, starting at the login home) — and
//! Connect. The SSH provision+launch runs on a worker thread
//! (`CodePuppy::spawn_remote`); the result is adopted into the Supervisor
//! from the drain loop (`remote_upkeep`), exactly like egui's
//! `poll_remote`. The blocking listing body is shared with egui
//! (`remote_connect::list_remote_blocking` — Phase E extraction).
//!
//! The folder-listing panel is the E5 path-browser port: folders-first
//! alphabetical, ".. up", monospace cwd header, "Use this folder". Only
//! the dir-pick mode is built here — egui's file-pick mode backs its
//! local @file picker, which the GPUI shell already covers (B2).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui::prelude::*;

use crate::backend::ssh::{self, SshTarget};
use crate::backend::{CodePuppy, UiEvent};
use crate::git::WorkspaceGit;
use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::{RootView, Screen};
use crate::views::remote_connect::{ListResult, join_remote, list_remote_blocking, parent_remote};
use crate::workspace::fs::WorkspaceFs;

/// What a remote-connect worker thread sends back (egui `RemoteSpawn`).
type RemoteSpawn = Result<
    (
        CodePuppy,
        Receiver<UiEvent>,
        Arc<dyn WorkspaceFs>,
        Arc<dyn WorkspaceGit>,
    ),
    String,
>;

/// An SSH connection being established off-thread + what adoption needs.
pub(crate) struct RemotePending {
    rx: Receiver<RemoteSpawn>,
    /// The remote folder path (becomes the workspace root).
    root: PathBuf,
    /// `user@host` for display.
    label: String,
    /// The full target — kept so the workspace can open further ssh
    /// channels (the embedded terminal, B13.7).
    target: SshTarget,
}

/// The remote folder browser, when open (egui `DirBrowser`).
pub(crate) struct DirBrowser {
    pub(crate) target: SshTarget,
    /// Absolute path currently shown (resolved by the remote `pwd`).
    pub(crate) cwd: String,
    pub(crate) entries: Vec<(String, bool)>,
    pub(crate) pending: Option<Receiver<ListResult>>,
    pub(crate) error: Option<String>,
}

/// Open state for the remote-connect dialog (egui `RemoteConnect`; the
/// target/path text lives in the two dedicated `ChatInput` entities).
pub(crate) struct RemoteState {
    /// Host aliases from `~/.ssh/config`, cached when the dialog opens.
    pub(crate) hosts: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) connecting: bool,
    pub(crate) browser: Option<DirBrowser>,
    /// Armed "really send credentials?" confirm for the dialog's puppush
    /// button (two-step, like the tree-delete confirm).
    pub(crate) push_confirm: bool,
}

/// Remote-connect interactions, nested under `DashAction::Remote`.
#[derive(Clone, Debug)]
pub enum RemoteAction {
    Open,
    Close,
    /// Click a config host: seed the target field.
    HostPick(String),
    Connect,
    /// "Push my auth + models to this host" — first dispatch arms the
    /// confirm, the second actually sends (it's credentials).
    PushCreds,
    PushCredsCancel,
    BrowseOpen,
    BrowseUp,
    BrowseEnter(String),
    /// "Use this folder" — current browser dir into the path field.
    BrowsePick,
    BrowseCancel,
}

/// An in-flight "puppush": auth + model config going to a remote host
/// (`backend::creds_push`). `ws` is set when launched from a workspace
/// toolbar so per-file results also land in that transcript.
pub(crate) struct CredsPush {
    pub(crate) label: String,
    pub(crate) ws: Option<crate::workspace::WorkspaceId>,
    pub(crate) rx: Receiver<Vec<(&'static str, crate::backend::creds_push::PushOutcome)>>,
}

/// Run the blocking push on a worker thread, waking the drain loop when done.
pub(crate) fn spawn_push(
    waker: Arc<dyn crate::waker::UiWaker>,
    target: SshTarget,
) -> Receiver<Vec<(&'static str, crate::backend::creds_push::PushOutcome)>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::backend::creds_push::push_creds_blocking(&target));
        waker.wake();
    });
    rx
}

/// List a remote directory off-thread, waking the drain loop when done.
fn spawn_list(
    waker: Arc<dyn crate::waker::UiWaker>,
    target: SshTarget,
    dir: Option<String>,
) -> Receiver<ListResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(list_remote_blocking(&target, dir.as_deref()));
        waker.wake();
    });
    rx
}

impl RootView {
    pub(crate) fn dispatch_remote(&mut self, action: RemoteAction, cx: &mut gpui::Context<Self>) {
        match action {
            RemoteAction::Open => {
                if self.remote.is_some() {
                    return; // already open (egui: open_remote && remote.is_none())
                }
                self.ensure_remote_inputs(cx);
                self.seed_remote(0, String::new(), cx);
                self.seed_remote(1, String::new(), cx);
                self.remote = Some(RemoteState {
                    hosts: ssh::config_hosts(),
                    error: None,
                    connecting: false,
                    browser: None,
                    push_confirm: false,
                });
            }
            RemoteAction::Close => {
                // egui ignores dismissal while the connection is in flight.
                if !self.remote.as_ref().is_some_and(|s| s.connecting) {
                    self.remote = None;
                }
            }
            RemoteAction::HostPick(host) => self.seed_remote(0, host, cx),
            RemoteAction::Connect => {
                let target_text = self.remote_text(0, cx);
                let path = self.remote_text(1, cx).trim().to_string();
                let Some(st) = &mut self.remote else { return };
                if st.connecting || target_text.trim().is_empty() || path.is_empty() {
                    return;
                }
                match SshTarget::parse(target_text.trim()) {
                    Ok(target) => {
                        st.error = None;
                        st.connecting = true;
                        self.begin_remote_connect(target, path);
                    }
                    Err(e) => st.error = Some(e),
                }
            }
            RemoteAction::PushCreds => {
                if self.creds_pending.is_some() {
                    return; // one push at a time
                }
                let target_text = self.remote_text(0, cx);
                let waker = self.waker.clone();
                let Some(st) = &mut self.remote else { return };
                match SshTarget::parse(target_text.trim()) {
                    Ok(target) => {
                        st.error = None;
                        if !st.push_confirm {
                            st.push_confirm = true; // arm: it's credentials
                            return;
                        }
                        st.push_confirm = false;
                        self.creds_pending = Some(CredsPush {
                            label: target.destination(),
                            ws: None,
                            rx: spawn_push(waker, target),
                        });
                    }
                    Err(e) => st.error = Some(e),
                }
            }
            RemoteAction::PushCredsCancel => {
                if let Some(st) = &mut self.remote {
                    st.push_confirm = false;
                }
            }
            RemoteAction::BrowseOpen => {
                let target_text = self.remote_text(0, cx);
                let waker = self.waker.clone();
                let Some(st) = &mut self.remote else { return };
                match SshTarget::parse(target_text.trim()) {
                    Ok(target) => {
                        st.error = None;
                        let pending = spawn_list(waker, target.clone(), None);
                        st.browser = Some(DirBrowser {
                            target,
                            cwd: "~".to_string(),
                            entries: Vec::new(),
                            pending: Some(pending),
                            error: None,
                        });
                    }
                    Err(e) => st.error = Some(format!("Enter a valid SSH target first: {e}")),
                }
            }
            RemoteAction::BrowseEnter(name) => {
                let waker = self.waker.clone();
                if let Some(b) = self.remote.as_mut().and_then(|s| s.browser.as_mut()) {
                    let dir = join_remote(&b.cwd, &name);
                    b.entries.clear();
                    b.pending = Some(spawn_list(waker, b.target.clone(), Some(dir)));
                }
            }
            RemoteAction::BrowseUp => {
                let waker = self.waker.clone();
                if let Some(b) = self.remote.as_mut().and_then(|s| s.browser.as_mut())
                    && let Some(parent) = parent_remote(&b.cwd)
                {
                    b.entries.clear();
                    b.pending = Some(spawn_list(waker, b.target.clone(), Some(parent)));
                }
            }
            RemoteAction::BrowsePick => {
                let picked = self
                    .remote
                    .as_ref()
                    .and_then(|s| s.browser.as_ref())
                    .map(|b| b.cwd.clone());
                if let Some(dir) = picked {
                    self.seed_remote(1, dir, cx);
                    if let Some(st) = &mut self.remote {
                        st.browser = None;
                    }
                }
            }
            RemoteAction::BrowseCancel => {
                if let Some(st) = &mut self.remote {
                    st.browser = None;
                }
            }
        }
        cx.notify();
    }

    /// Provision + launch the remote sidecar on a worker thread (egui
    /// `begin_remote_connect`); the result lands via `remote_upkeep`.
    fn begin_remote_connect(&mut self, target: SshTarget, remote_path: String) {
        if self.remote_pending.is_some() {
            return;
        }
        let label = target.destination();
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = self.waker.clone();
        let path = remote_path.clone();
        let worker_target = target.clone();
        std::thread::spawn(move || {
            let result = CodePuppy::spawn_remote(waker.clone(), &worker_target, Some(&path));
            let _ = tx.send(result);
            waker.wake();
        });
        self.remote_pending = Some(RemotePending {
            rx,
            root: PathBuf::from(remote_path),
            label,
            target,
        });
    }

    /// Drain-loop upkeep: poll the in-flight listing + connection threads
    /// (egui's per-frame `try_recv` + `poll_remote`).
    /// Poll the in-flight creds push: toast the summary, and when launched
    /// from a workspace also leave the per-file detail in its transcript
    /// (file NAMES and outcomes only — contents are never logged).
    fn creds_upkeep(&mut self) {
        use crate::backend::creds_push::PushOutcome;
        let Some(p) = &self.creds_pending else { return };
        let results = match p.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.creds_pending = None;
                return;
            }
        };
        let p = self.creds_pending.take().expect("checked above");
        let summary = crate::backend::creds_push::summarize(&results);
        let failed = results
            .iter()
            .any(|(_, o)| matches!(o, PushOutcome::Failed(_)));
        let color = if failed {
            self.tokens.error
        } else {
            self.tokens.accent
        };
        self.toast(format!("Creds \u{2192} {}: {summary}", p.label), color);
        if let Some(id) = p.ws
            && let Some(ws) = self.supervisor.get_mut(id)
        {
            let mut lines = vec![format!("Pushed auth + models to {}:", p.label)];
            for (name, o) in &results {
                let what = match o {
                    PushOutcome::Pushed => "pushed".to_string(),
                    PushOutcome::Missing => "skipped (not present locally)".to_string(),
                    PushOutcome::Failed(e) => format!("FAILED: {e}"),
                };
                lines.push(format!("  {name}: {what}"));
            }
            ws.push_note(lines.join("\n"));
        }
    }

    pub(crate) fn remote_upkeep(&mut self, cx: &mut gpui::Context<Self>) {
        self.creds_upkeep();
        // Folder-browser listing result.
        if let Some(b) = self.remote.as_mut().and_then(|s| s.browser.as_mut())
            && let Some(rx) = &b.pending
        {
            match rx.try_recv() {
                Ok(Ok((cwd, entries))) => {
                    b.cwd = cwd;
                    b.entries = entries;
                    b.error = None;
                    b.pending = None;
                }
                Ok(Err(e)) => {
                    b.error = Some(e);
                    b.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => b.pending = None,
            }
        }
        // Connection result -> adopt into the Supervisor (egui poll_remote).
        let Some(pending) = self.remote_pending.take() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => {
                self.remote_pending = Some(pending); // still in flight
                return;
            }
            Err(TryRecvError::Disconnected) => return,
        };
        match result {
            Ok((backend, ev_rx, fs, git)) => {
                let accent = self.tokens.accent;
                let id = self.supervisor.adopt(
                    pending.root,
                    Some(crate::workspace::RemoteInfo {
                        label: pending.label.clone(),
                        target: pending.target.clone(),
                    }),
                    fs,
                    git,
                    backend,
                    ev_rx,
                );
                // egui pushes a Chat tab for the new workspace; our shape
                // of that is jumping to its chat screen. The input MUST
                // exist before the screen flips — render assumes it
                // (B13.1: skipping ensure_chat_input here crashed the app).
                self.ensure_chat_input(id, cx);
                self.screen = Screen::Chat(id);
                self.pending_focus = Some(id);
                self.toast(format!("Connected {}", pending.label), accent);
                self.remote = None; // close the dialog
            }
            Err(e) => {
                // Keep the dialog open and show the failure inline.
                let error_color = self.tokens.error;
                if let Some(st) = self.remote.as_mut() {
                    st.connecting = false;
                    st.error = Some(e);
                } else {
                    self.toast(format!("Remote connect failed: {e}"), error_color);
                }
            }
        }
    }

    fn ensure_remote_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        if self.remote_inputs.is_empty() {
            for hint in ["alice@devbox   or   devbox", "/home/alice/project"] {
                let entity = cx.new(|cx| ChatInput::new(hint, cx));
                let sub = cx.subscribe(&entity, |_, _, _: &crate::gpui_ui::InputEvent, cx| {
                    cx.notify()
                });
                self.remote_inputs.push(entity);
                self.chat_subs.push(sub);
            }
        }
    }

    fn remote_text(&self, ix: usize, cx: &gpui::Context<Self>) -> String {
        self.remote_inputs
            .get(ix)
            .map(|i| i.read(cx).text().to_string())
            .unwrap_or_default()
    }

    fn seed_remote(&self, ix: usize, text: String, cx: &mut gpui::Context<Self>) {
        if let Some(input) = self.remote_inputs.get(ix) {
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }
}
