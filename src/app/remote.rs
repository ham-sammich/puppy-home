//! Remote-workspace connection plumbing for [`PuppyApp`].
//!
//! Establishing an SSH connection (provision + launch the sidecar) can take a
//! few seconds, so it runs on a worker thread; the result is adopted into a
//! workspace on the UI thread via [`PuppyApp::poll_remote`]. The dialog itself
//! lives in `crate::views::remote_connect`.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;

use crate::backend::ssh::SshTarget;
use crate::backend::{CodePuppy, UiEvent};
use crate::shell::Tab;

use super::PuppyApp;

/// The result a remote-connect worker thread sends back: a spawned sidecar +
/// its event stream, or an error string.
pub(super) type RemoteSpawn = Result<(CodePuppy, Receiver<UiEvent>), String>;

/// A remote connection being established off-thread, plus what we need to adopt
/// the workspace once it lands.
pub(super) struct RemotePending {
    pub(super) rx: Receiver<RemoteSpawn>,
    /// The remote folder path (becomes the workspace root).
    pub(super) root: PathBuf,
    /// `user@host` for display.
    pub(super) label: String,
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
        let ctx2 = ctx.clone();
        let path = remote_path.clone();
        std::thread::spawn(move || {
            let result = CodePuppy::spawn_remote(ctx2.clone(), &target, Some(&path));
            let _ = tx.send(result);
            ctx2.request_repaint();
        });
        self.remote_pending = Some(RemotePending {
            rx,
            root: PathBuf::from(remote_path),
            label,
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
            Ok((backend, ev_rx)) => {
                let id = self
                    .sup
                    .adopt(pending.root, Some(pending.label), backend, ev_rx);
                if let Some(dock) = self.dock.as_mut() {
                    dock.push_to_focused_leaf(Tab::Chat(id));
                }
                self.remote = None; // close the dialog
                self.status.clear();
            }
            Err(e) => {
                // Keep the dialog open and show the failure inline.
                if let Some(st) = self.remote.as_mut() {
                    st.connecting = false;
                    st.error = Some(e);
                } else {
                    self.status = format!("Remote connect failed: {e}");
                }
            }
        }
    }
}
