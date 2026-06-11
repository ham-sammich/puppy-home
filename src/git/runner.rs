//! How git is actually executed for a repo. The parsing in `git.rs` is shared;
//! only execution differs: [`LocalRunner`] shells out to `git -C <root>`, while
//! the backend's `RemoteRunner` ships the same args over the sidecar RPC.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Executes `git` for one repo. The free functions in `git.rs` parse what it
/// returns, so a local and a remote runner behave identically.
pub(crate) trait GitRunner {
    /// Run `git <args>`; stdout on success, a trimmed error on failure.
    fn run(&self, args: &[&str]) -> Result<String, String>;
    /// Like [`run`](Self::run) but with extra environment variables set (used to
    /// feed credentials to an authenticated network op).
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, String>;
    /// Run `git <args>`; stdout regardless of exit status (empty on failure).
    fn output(&self, args: &[&str]) -> String;
    /// Read a working-tree file by repo-relative path (for untracked diffs).
    fn read_workfile(&self, rel: &str) -> Option<String>;
}

/// Runs git on the local machine via `git -C <root>`.
pub(crate) struct LocalRunner {
    root: PathBuf,
}

impl LocalRunner {
    pub(crate) fn new(root: PathBuf) -> Self {
        LocalRunner { root }
    }

    fn git(&self) -> Command {
        let mut c = Command::new("git");
        crate::proc::hide_console(&mut c);
        c.arg("-C").arg(&self.root);
        // Never block on a tty credential prompt -- fail fast so the GUI can
        // pop a credentials modal instead. (Credential *helpers* still run.)
        c.env("GIT_TERMINAL_PROMPT", "0");
        c
    }
}

impl GitRunner for LocalRunner {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
        let mut cmd = self.git();
        for (k, v) in env {
            cmd.env(k, v);
        }
        match cmd.args(args).stdin(Stdio::null()).output() {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let err = err.trim();
                Err(if err.is_empty() {
                    format!("git {} failed", args.first().copied().unwrap_or(""))
                } else {
                    err.to_string()
                })
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn output(&self, args: &[&str]) -> String {
        self.git()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    fn read_workfile(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }
}
