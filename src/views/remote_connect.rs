//! "Connect to a remote folder" dialog.
//!
//! Lets the user pick a host discovered from their `~/.ssh/config` (or type a
//! `[user@]host[:port]` target by hand), choose a remote folder -- typed or via
//! the built-in folder browser (SSH `ls`, starting at the login home) -- and
//! open a workspace whose Code Puppy sidecar runs on that host over SSH. The
//! actual connection happens off-thread in the app; this is just the form.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use crate::backend::ssh::{self, SshTarget};
use crate::views::path_browser::{self, BrowseAction};

/// A remote directory listing in flight or done: `(resolved_abs_path, entries)`.
/// (pub(crate): the GPUI connect dialog polls the same result type.)
pub(crate) type ListResult = Result<(String, Vec<(String, bool)>), String>;

/// Run one remote `ls` and parse it — the blocking body shared by both
/// shells' off-thread listing spawns (sync note: extracted in Phase E).
/// `dir` of `None` lists the login home.
pub(crate) fn list_remote_blocking(target: &SshTarget, dir: Option<&str>) -> ListResult {
    match target.list_dir_command(dir).output() {
        Ok(o) if o.status.success() => ssh::parse_listing(&String::from_utf8_lossy(&o.stdout))
            .ok_or_else(|| "Couldn't read that directory.".to_string()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let err = err.trim();
            Err(if err.is_empty() {
                "Listing failed.".to_string()
            } else {
                err.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// The remote folder browser state (opened from "Browse the remote host").
struct DirBrowser {
    target: SshTarget,
    /// Absolute path currently shown (resolved by the remote `pwd`).
    cwd: String,
    entries: Vec<(String, bool)>,
    /// In-flight listing, if a request is running.
    pending: Option<Receiver<ListResult>>,
    error: Option<String>,
}

/// Open state for the remote-connect modal.
pub struct RemoteConnect {
    /// Free-text `[user@]host[:port]` (also filled by clicking a config host).
    pub target: String,
    /// Remote folder path the sidecar runs in.
    pub path: String,
    /// Host aliases discovered from `~/.ssh/config` (cached when the dialog opens).
    hosts: Vec<String>,
    /// Last validation / connection error to surface in the form.
    pub error: Option<String>,
    /// True while the SSH connection is being established off-thread.
    pub connecting: bool,
    /// Set when connect failed with CannotHost (the missing launcher's
    /// name): the dialog offers SSH-fallback mode — explicit opt-in only.
    pub fallback_offer: Option<String>,
    /// Active remote folder browser, when open.
    browser: Option<DirBrowser>,
}

impl RemoteConnect {
    #[allow(clippy::new_without_default)] // reads ssh config; not a cheap Default
    pub fn new() -> Self {
        RemoteConnect {
            target: String::new(),
            path: String::new(),
            hosts: ssh::config_hosts(),
            error: None,
            connecting: false,
            fallback_offer: None,
            browser: None,
        }
    }
}

/// What the dialog wants the app to do this frame.
#[derive(Default)]
pub struct Outcome {
    /// Validated target + remote path to connect to.
    pub connect: Option<(SshTarget, String)>,
    /// The user accepted the SSH-fallback offer (target + remote path).
    pub fallback: Option<(SshTarget, String)>,
    /// The user dismissed the dialog.
    pub cancel: bool,
}

/// List a remote directory off-thread, waking the UI when it lands. `dir` of
/// `None` lists the login home.
fn spawn_list(ctx: &egui::Context, target: SshTarget, dir: Option<String>) -> Receiver<ListResult> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(list_remote_blocking(&target, dir.as_deref()));
        ctx.request_repaint();
    });
    rx
}

/// Join a remote dir + child name without doubling the separator.
pub(crate) fn join_remote(cwd: &str, name: &str) -> String {
    if cwd.ends_with('/') {
        format!("{cwd}{name}")
    } else {
        format!("{cwd}/{name}")
    }
}

/// Parent of an absolute remote path (`None` at the root).
pub(crate) fn parent_remote(cwd: &str) -> Option<String> {
    if cwd == "/" {
        return None;
    }
    match cwd.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => Some("/".to_string()),
        Some((parent, _)) => Some(parent.to_string()),
        None => None,
    }
}

/// Render the modal; returns the requested action(s) for this frame.
pub fn render(ctx: &egui::Context, st: &mut RemoteConnect) -> Outcome {
    let mut out = Outcome::default();
    let mut window_open = true;
    egui::Window::new("Connect to a remote folder")
        .id(egui::Id::new("remote-connect"))
        .collapsible(false)
        .resizable(false)
        .open(&mut window_open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(440.0);

            if st.hosts.is_empty() {
                ui.weak("No hosts found in ~/.ssh/config.");
            } else {
                ui.label("Hosts from your SSH config:");
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .auto_shrink([false, false])
                    .id_salt("remote-hosts")
                    .show(ui, |ui| {
                        for host in &st.hosts {
                            let selected = st.target == *host;
                            if ui.selectable_label(selected, host.as_str()).clicked() {
                                st.target = host.clone();
                            }
                        }
                    });
            }
            ui.add_space(8.0);

            ui.label("SSH target  ( [user@]host[:port] ):");
            ui.add(
                egui::TextEdit::singleline(&mut st.target)
                    .desired_width(f32::INFINITY)
                    .hint_text("alice@devbox   or   devbox"),
            );
            ui.add_space(6.0);

            if st.browser.is_some() {
                render_browser(ctx, ui, st);
            } else {
                render_path_field(ctx, ui, st);
            }

            if let Some(err) = &st.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, err);
            }

            // CannotHost verdict: offer SSH-fallback mode explicitly
            // (mirrors the gpui shell's offer — never automatic).
            if let Some(launcher) = st.fallback_offer.clone() {
                ui.add_space(8.0);
                ui.label(format!(
                    "The remote can't run Code Puppy (`{launcher}` not found). \
                     Fallback: the agent runs LOCALLY and operates on the \
                     remote project over ssh — slower, and tools run on this \
                     machine, not the remote."
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Use SSH fallback (limited)").clicked() {
                        match SshTarget::parse(st.target.trim()) {
                            Ok(target) => {
                                out.fallback = Some((target, st.path.trim().to_string()));
                            }
                            Err(e) => st.error = Some(e),
                        }
                    }
                    if ui.button("Back").clicked() {
                        st.fallback_offer = None;
                    }
                });
                return;
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if st.connecting {
                    ui.spinner();
                    ui.label("Connecting over SSH…");
                    return;
                }
                if ui.button("Cancel").clicked() {
                    out.cancel = true;
                }
                let ready = !st.target.trim().is_empty() && !st.path.trim().is_empty();
                if ui
                    .add_enabled(ready, egui::Button::new("Connect"))
                    .clicked()
                {
                    match SshTarget::parse(st.target.trim()) {
                        Ok(target) => {
                            st.error = None;
                            out.connect = Some((target, st.path.trim().to_string()));
                        }
                        Err(e) => st.error = Some(e),
                    }
                }
            });
        });
    if !window_open {
        out.cancel = true;
    }
    out
}

/// The typed-path row + "Browse" button (browser closed).
fn render_path_field(ctx: &egui::Context, ui: &mut egui::Ui, st: &mut RemoteConnect) {
    ui.label("Remote folder path:");
    ui.add(
        egui::TextEdit::singleline(&mut st.path)
            .desired_width(f32::INFINITY)
            .hint_text("/home/alice/project"),
    );
    ui.add_space(4.0);
    if ui
        .button("Browse the remote host…")
        .on_hover_text("Pick a folder by browsing the remote filesystem")
        .clicked()
    {
        match SshTarget::parse(st.target.trim()) {
            Ok(target) => {
                st.error = None;
                let pending = spawn_list(ctx, target.clone(), None);
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
}

/// The active folder browser: poll the listing, draw it, handle navigation.
fn render_browser(ctx: &egui::Context, ui: &mut egui::Ui, st: &mut RemoteConnect) {
    let mut picked: Option<String> = None;
    let mut close = false;

    if let Some(browser) = &mut st.browser {
        if let Some(rx) = &browser.pending {
            match rx.try_recv() {
                Ok(Ok((cwd, entries))) => {
                    browser.cwd = cwd;
                    browser.entries = entries;
                    browser.error = None;
                    browser.pending = None;
                }
                Ok(Err(e)) => {
                    browser.error = Some(e);
                    browser.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => browser.pending = None,
            }
        }
        let loading = browser.pending.is_some();
        ui.label("Pick a remote folder:");
        let action = path_browser::render_listing(
            ui,
            &browser.cwd,
            &browser.entries,
            true,
            loading,
            browser.error.as_deref(),
        );
        match action {
            Some(BrowseAction::Enter(name)) => {
                let dir = join_remote(&browser.cwd, &name);
                browser.entries.clear();
                browser.pending = Some(spawn_list(ctx, browser.target.clone(), Some(dir)));
            }
            Some(BrowseAction::Up) => {
                if let Some(parent) = parent_remote(&browser.cwd) {
                    browser.entries.clear();
                    browser.pending = Some(spawn_list(ctx, browser.target.clone(), Some(parent)));
                }
            }
            Some(BrowseAction::Pick(None)) => picked = Some(browser.cwd.clone()),
            Some(BrowseAction::Pick(Some(_))) | None => {}
        }
        ui.add_space(4.0);
        if ui.button("Cancel browse").clicked() {
            close = true;
        }
    }

    if let Some(dir) = picked {
        st.path = dir;
        st.browser = None;
    } else if close {
        st.browser = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_remote_never_doubles_the_separator() {
        assert_eq!(join_remote("/home/alice", "src"), "/home/alice/src");
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("/var/", "log"), "/var/log");
    }

    #[test]
    fn parent_remote_walks_to_root_and_stops() {
        assert_eq!(parent_remote("/a/b/c"), Some("/a/b".to_string()));
        assert_eq!(parent_remote("/a"), Some("/".to_string()));
        assert_eq!(parent_remote("/a/"), Some("/".to_string()));
        assert_eq!(parent_remote("/"), None);
    }
}
