//! Remote-workspace connection plumbing for [`PuppyApp`].
//!
//! Establishing an SSH connection (provision + launch the sidecar) can take a
//! few seconds, so it runs on a worker thread; the result is adopted into a
//! workspace on the UI thread via [`PuppyApp::poll_remote`]. The dialog itself
//! lives in `crate::views::remote_connect`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;

use crate::backend::ssh::SshTarget;
use crate::backend::{CodePuppy, RemoteError, UiEvent};
use crate::git::WorkspaceGit;
use crate::shell::Tab;
use crate::workspace::fs::WorkspaceFs;

use super::PuppyApp;

/// The result a remote-connect worker thread sends back: a spawned sidecar, its
/// event stream, and the remote filesystem + git handles -- or an error string.
pub(super) type RemoteSpawn = Result<
    (
        CodePuppy,
        Receiver<UiEvent>,
        Arc<dyn WorkspaceFs>,
        Arc<dyn WorkspaceGit>,
    ),
    RemoteError,
>;
/// A remote connection being established off-thread, plus what we need to adopt
/// the workspace once it lands.
pub(super) struct RemotePending {
    pub(super) rx: Receiver<RemoteSpawn>,
    /// The remote folder path (becomes the workspace root).
    pub(super) root: PathBuf,
    /// `user@host` for display.
    pub(super) label: String,
    /// The full target — kept so the workspace can open further ssh
    /// channels (the embedded terminal, B13.7).
    pub(super) target: SshTarget,
    /// True when this pending spawn is SSH-fallback mode (CannotHost +
    /// explicit user opt-in).
    pub(super) fallback: bool,
}

impl PuppyApp {
    /// Establish a remote SSH connection on a worker thread (provision + launch
    /// the sidecar). The UI never blocks; the result arrives via `poll_remote`.
    pub(super) fn begin_remote_connect(
        &mut self,
        target: SshTarget,
        remote_path: String,
        ctx: &egui::Context,
    ) {
        if self.remote_pending.is_some() {
            return;
        }
        let label = target.destination();
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = crate::waker::egui_waker(ctx);
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
            fallback: false,
        });
    }

    /// Spawn SSH-FALLBACK mode on a worker thread: the sidecar runs LOCALLY
    /// against a scratch cwd instructed to operate on the remote over ssh
    /// (see `backend::ssh_fallback`). Only offered after a CannotHost
    /// verdict and explicit user opt-in — never automatic.
    pub(super) fn begin_remote_fallback(
        &mut self,
        target: SshTarget,
        remote_path: String,
        ctx: &egui::Context,
    ) {
        if self.remote_pending.is_some() {
            return;
        }
        let label = target.destination();
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = crate::waker::egui_waker(ctx);
        let path = remote_path.clone();
        let worker_target = target.clone();
        std::thread::spawn(move || {
            let result = CodePuppy::spawn_ssh_fallback(waker.clone(), &worker_target, &path)
                .map_err(RemoteError::Other);
            let _ = tx.send(result);
            waker.wake();
        });
        self.remote_pending = Some(RemotePending {
            rx,
            root: PathBuf::from(remote_path),
            label,
            target,
            fallback: true,
        });
    }

    pub(super) fn poll_remote(&mut self) {
        // Pull the worker result without holding a borrow across the mutation.
        let result = match &self.remote_pending {
            Some(p) => match p.rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.remote_pending = None;
                    return;
                }
            },
            None => return,
        };
        let pending = self.remote_pending.take().expect("pending present");
        match result {
            Ok((backend, ev_rx, fs, git)) => {
                let id = self.sup.adopt(
                    pending.root,
                    Some(crate::workspace::RemoteInfo {
                        label: pending.label,
                        target: pending.target,
                        fallback: pending.fallback,
                    }),
                    fs,
                    git,
                    backend,
                    ev_rx,
                );
                if let Some(dock) = self.dock.as_mut() {
                    dock.push_to_focused_leaf(Tab::Chat(id));
                }
                self.remote = None; // close the dialog
                self.status.clear();
            }
            Err(e) => {
                // Keep the dialog open; CannotHost gets the explicit
                // SSH-fallback offer, everything else shows inline.
                if let Some(st) = self.remote.as_mut() {
                    st.connecting = false;
                    match e {
                        RemoteError::CannotHost { launcher } => {
                            st.error = None;
                            st.fallback_offer = Some(launcher);
                        }
                        RemoteError::Other(e) => st.error = Some(e),
                    }
                } else {
                    self.status = format!("Remote connect failed: {e}");
                }
            }
        }
    }
}
