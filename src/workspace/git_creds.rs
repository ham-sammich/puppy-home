//! The HTTPS-credentials flow for git network ops.
//!
//! The sidecar runs git non-interactively, so an HTTPS remote that wants a
//! username/password (or PAT) can't prompt -- the op fails fast (we set
//! `GIT_TERMINAL_PROMPT=0`). When that happens we pop a modal, collect the
//! credentials, and retry via the `*_auth` ops, which feed them to git through a
//! one-shot credential helper. Nothing is persisted.

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

    /// The outstanding credentials prompt, if any (frontends render it).
    pub(crate) fn git_creds_prompt(&self) -> Option<&GitCredsPrompt> {
        self.git_creds.as_ref()
    }

    /// Submit credentials from a frontend (sets the prompt fields + retries).
    pub(crate) fn git_creds_submit(&mut self, username: String, password: String) {
        if let Some(p) = self.git_creds.as_mut() {
            p.username = username;
            p.password = password;
        }
        self.retry_git_auth();
    }

    /// Dismiss the prompt without retrying.
    pub(crate) fn git_creds_cancel(&mut self) {
        self.git_creds = None;
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
}
