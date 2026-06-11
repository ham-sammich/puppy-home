//! "Connect to a remote folder" dialog.
//!
//! Lets the user pick a host discovered from their `~/.ssh/config` (or type a
//! `[user@]host[:port]` target by hand), give a remote folder path, and open a
//! workspace whose Code Puppy sidecar runs on that host over SSH. The actual
//! connection happens off-thread in the app; this is just the form.

use eframe::egui;

use crate::backend::ssh::{self, SshTarget};

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
        }
    }
}

/// What the dialog wants the app to do this frame.
#[derive(Default)]
pub struct Outcome {
    /// Validated target + remote path to connect to.
    pub connect: Option<(SshTarget, String)>,
    /// The user dismissed the dialog.
    pub cancel: bool,
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
            ui.set_min_width(420.0);

            if st.hosts.is_empty() {
                ui.weak("No hosts found in ~/.ssh/config.");
            } else {
                ui.label("Hosts from your SSH config:");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .auto_shrink([false, false])
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
            ui.add_space(4.0);
            ui.label("Remote folder path:");
            ui.add(
                egui::TextEdit::singleline(&mut st.path)
                    .desired_width(f32::INFINITY)
                    .hint_text("/home/alice/project"),
            );

            if let Some(err) = &st.error {
                ui.add_space(6.0);
                ui.colored_label(ui.visuals().error_fg_color, err);
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
