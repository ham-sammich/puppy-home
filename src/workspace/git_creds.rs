//! The HTTPS-credentials flow for git network ops.
//!
//! The sidecar runs git non-interactively, so an HTTPS remote that wants a
//! username/password (or PAT) can't prompt -- the op fails fast (we set
//! `GIT_TERMINAL_PROMPT=0`). When that happens we pop a modal, collect the
//! credentials, and retry via the `*_auth` ops, which feed them to git through a
//! one-shot credential helper. Nothing is persisted.

use eframe::egui;

use super::Workspace;
use super::state::{GitAuthOp, GitCredsPrompt};

impl Workspace {
    /// Handle a failed network op: if the remote wants HTTPS credentials, pop the
    /// credentials modal; otherwise just report the error like any git action.
    pub(crate) fn git_net_error(&mut self, err: String, op: GitAuthOp) {
        if crate::git::is_auth_error(&err) {
            self.git_creds = Some(GitCredsPrompt {
                op,
                username: String::new(),
                password: String::new(),
                error: None,
                focus: true,
            });
        } else {
            self.git_action(Err(err), "");
        }
    }

    /// Retry the modal's op with the entered credentials. On success the modal
    /// closes; on another auth failure it stays open for a second try.
    fn retry_git_auth(&mut self) {
        let Some((op, user, pass)) = self
            .git_creds
            .as_ref()
            .map(|p| (p.op, p.username.clone(), p.password.clone()))
        else {
            return;
        };
        let result = match op {
            GitAuthOp::Fetch => self.git.fetch_auth(&user, &pass),
            GitAuthOp::Pull => self.git.pull_auth(&user, &pass),
            GitAuthOp::Push => self.git.push_auth(&user, &pass),
        };
        match result {
            Ok(s) => {
                self.git_creds = None;
                let msg = match op {
                    GitAuthOp::Fetch => "Fetched from remotes".to_string(),
                    GitAuthOp::Pull => format!("Pulled · {}", s.lines().last().unwrap_or("Pulled")),
                    GitAuthOp::Push => "Pushed to upstream".to_string(),
                };
                self.git_action(Ok(()), &msg);
            }
            Err(e) => {
                if let Some(p) = self.git_creds.as_mut() {
                    p.error = Some(if crate::git::is_auth_error(&e) {
                        "Authentication failed — check your username/token.".to_string()
                    } else {
                        e
                    });
                    p.password.clear();
                    p.focus = true;
                }
            }
        }
    }

    /// The HTTPS credentials modal shown when a push/pull/fetch needs auth.
    pub(crate) fn render_git_creds_modal(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.git_creds.take() else {
            return;
        };
        let verb = state.op.verb();
        let mut action = 0u8; // 1 = authenticate, 2 = cancel
        egui::Window::new(format!("{verb} — sign in"))
            .id(egui::Id::new(("git-creds-modal", self.id.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(format!(
                    "The remote needs a username and password (or token) to {}.",
                    verb.to_ascii_lowercase()
                ));
                ui.add_space(8.0);
                egui::Grid::new("git-creds-grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Username:");
                        let user_resp = ui.text_edit_singleline(&mut state.username);
                        if state.focus {
                            user_resp.request_focus();
                            state.focus = false;
                        }
                        ui.end_row();
                        ui.label("Password / token:");
                        let pass_resp =
                            ui.add(egui::TextEdit::singleline(&mut state.password).password(true));
                        ui.end_row();
                        // Enter in either field submits.
                        let enter = (user_resp.lost_focus() || pass_resp.lost_focus())
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if enter {
                            action = 1;
                        }
                    });
                ui.weak("Used once for this push — nothing is stored.");
                if let Some(err) = &state.error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = 2;
                    }
                    let ready = !state.username.trim().is_empty() && !state.password.is_empty();
                    if ui.add_enabled(ready, egui::Button::new(verb)).clicked() {
                        action = 1;
                    }
                });
            });
        match action {
            1 => {
                self.git_creds = Some(state);
                self.retry_git_auth();
            }
            2 => self.git_creds = None,
            _ => self.git_creds = Some(state),
        }
    }
}
