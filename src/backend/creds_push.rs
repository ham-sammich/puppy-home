//! "puppush" — push the LOCAL code_puppy auth + model config to a remote
//! host's `~/.code_puppy`, so a remote sidecar can use your providers
//! without re-running OAuth flows over there.
//!
//! The manifest below was derived from the code_puppy source (config.py +
//! the auth plugins), not guessed. Everything we push lives in code_puppy's
//! DATA_DIR (`$XDG_DATA_HOME/code_puppy` when XDG is explicitly set, else
//! the legacy `~/.code_puppy` — mirrored by [`local_data_dir`]).
//!
//! Deliberately EXCLUDED from the manifest:
//! * `puppy.cfg` — carries the puppy/owner identity (the remote keeps its
//!   own puppy, B13.8) AND plain API keys ride inside it; pushing it would
//!   clobber identity. Consequence, documented honestly: API-key-based
//!   providers (fireworks etc.) are NOT covered by a push — only the OAuth
//!   providers with dedicated token files.
//! * `mcp_servers.json` — machine-specific commands and paths.
//! * agents/skills/contexts dirs, caches, command history, terminal
//!   sessions — machine state, not credentials.
//!
//! Transfer matches the sidecar-provisioning convention: bytes piped over
//! `ssh` stdin (`mkdir -p && cat > file`), one call per file so the UI can
//! report per-file results. `BatchMode=yes` is right here — there is no
//! PTY, so we fail fast rather than hang on an auth prompt. Sensitive
//! files get `chmod 600` remotely (the same mode code_puppy's own plugins
//! set locally). File contents are never logged.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::ssh::SshTarget;

/// One pushable file: its name under the data dir + classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredFile {
    /// File name inside `~/.code_puppy` (same name locally and remotely).
    pub name: &'static str,
    /// Credentials/tokens: `chmod 600` on the remote.
    pub sensitive: bool,
}

/// Everything a push covers. Sources, from the code_puppy checkout:
/// * token files: `plugins/{claude_code_oauth,chatgpt_oauth}/config.py`,
///   `plugins/copilot_auth/config.py` (session + device tokens) — each
///   plugin chmods its file 0o600 locally.
/// * model files: `config.py` (`MODELS_FILE`, `EXTRA_MODELS_FILE`, and the
///   per-provider `*_MODELS_FILE` constants).
pub const MANIFEST: &[CredFile] = &[
    // SENSITIVE — OAuth tokens / sessions.
    CredFile {
        name: "claude_code_oauth.json",
        sensitive: true,
    },
    CredFile {
        name: "chatgpt_oauth.json",
        sensitive: true,
    },
    CredFile {
        name: "copilot_session.json",
        sensitive: true,
    },
    CredFile {
        name: "copilot_device_tokens.json",
        sensitive: true,
    },
    // Plain model config.
    CredFile {
        name: "models.json",
        sensitive: false,
    },
    CredFile {
        name: "extra_models.json",
        sensitive: false,
    },
    CredFile {
        name: "claude_models.json",
        sensitive: false,
    },
    CredFile {
        name: "chatgpt_models.json",
        sensitive: false,
    },
    CredFile {
        name: "copilot_models.json",
        sensitive: false,
    },
    CredFile {
        name: "gemini_models.json",
        sensitive: false,
    },
];

/// Per-file push outcome (surfaced verbatim in the UI feedback line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    Pushed,
    /// Not present locally — skipped with a note, never fatal.
    Missing,
    Failed(String),
}

/// The local code_puppy data dir, mirroring code_puppy's `_get_xdg_dir`:
/// XDG applies ONLY when the env var is explicitly set, else the legacy
/// `~/.code_puppy`.
pub fn local_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    data_dir_from(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from), &home)
}

/// Testable core of [`local_data_dir`].
fn data_dir_from(xdg_data_home: Option<PathBuf>, home: &Path) -> PathBuf {
    match xdg_data_home {
        Some(x) if !x.as_os_str().is_empty() => x.join("code_puppy"),
        _ => home.join(".code_puppy"),
    }
}

/// Push every manifest file that exists locally to `target`, blocking.
/// Run on a worker thread; returns per-file outcomes in manifest order
/// (existing files only produce Pushed/Failed, absent ones Missing).
pub fn push_creds_blocking(target: &SshTarget) -> Vec<(&'static str, PushOutcome)> {
    let dir = local_data_dir();
    MANIFEST
        .iter()
        .map(|f| (f.name, push_one(target, &dir, f)))
        .collect()
}

fn push_one(target: &SshTarget, dir: &Path, file: &CredFile) -> PushOutcome {
    let bytes = match std::fs::read(dir.join(file.name)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PushOutcome::Missing,
        Err(e) => return PushOutcome::Failed(format!("reading local file: {e}")),
    };
    let mut cmd = target.push_file_command(file.name, file.sensitive);
    crate::proc::hide_console(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return PushOutcome::Failed(format!("starting ssh: {e}")),
    };
    {
        let Some(mut si) = child.stdin.take() else {
            return PushOutcome::Failed("no stdin pipe".into());
        };
        if let Err(e) = si.write_all(&bytes) {
            return PushOutcome::Failed(format!("sending file: {e}"));
        }
        // drop -> EOF, the remote `cat` finishes.
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() => PushOutcome::Pushed,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.trim();
            PushOutcome::Failed(if err.is_empty() {
                "ssh exited nonzero".to_string()
            } else {
                err.to_string()
            })
        }
        Err(e) => PushOutcome::Failed(format!("waiting on ssh: {e}")),
    }
}

/// One-line summary for toasts: "pushed 3, skipped 7 (not present)" plus a
/// failure count when any.
pub fn summarize(results: &[(&'static str, PushOutcome)]) -> String {
    let pushed = results
        .iter()
        .filter(|(_, o)| *o == PushOutcome::Pushed)
        .count();
    let missing = results
        .iter()
        .filter(|(_, o)| *o == PushOutcome::Missing)
        .count();
    let failed: Vec<&str> = results
        .iter()
        .filter(|(_, o)| matches!(o, PushOutcome::Failed(_)))
        .map(|(n, _)| *n)
        .collect();
    let mut s = format!("pushed {pushed}, skipped {missing} (not present locally)");
    if !failed.is_empty() {
        s.push_str(&format!("; FAILED: {}", failed.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_classification_matches_code_puppy() {
        // Every token/session file is sensitive; every model file is not.
        for f in MANIFEST {
            let is_token = f.name.contains("oauth")
                || f.name.contains("session")
                || f.name.contains("device_tokens");
            assert_eq!(
                f.sensitive, is_token,
                "{} classification looks wrong",
                f.name
            );
        }
        // The four token files derived from the code_puppy plugins.
        assert_eq!(MANIFEST.iter().filter(|f| f.sensitive).count(), 4);
        // Identity/machine state never travels.
        for banned in ["puppy.cfg", "mcp_servers.json", "terminal_sessions.json"] {
            assert!(MANIFEST.iter().all(|f| f.name != banned));
        }
    }

    #[test]
    fn data_dir_mirrors_code_puppy_xdg_rule() {
        let home = Path::new("/home/alice");
        // XDG only when explicitly set...
        assert_eq!(
            data_dir_from(Some(PathBuf::from("/xdg/data")), home),
            PathBuf::from("/xdg/data/code_puppy")
        );
        // ...empty or unset falls back to the legacy dot-dir.
        assert_eq!(
            data_dir_from(Some(PathBuf::new()), home),
            home.join(".code_puppy")
        );
        assert_eq!(data_dir_from(None, home), home.join(".code_puppy"));
    }

    #[test]
    fn summarize_counts_and_failures() {
        let results = vec![
            ("a.json", PushOutcome::Pushed),
            ("b.json", PushOutcome::Missing),
            ("c.json", PushOutcome::Failed("boom".into())),
        ];
        let s = summarize(&results);
        assert!(s.contains("pushed 1"));
        assert!(s.contains("skipped 1"));
        assert!(s.contains("FAILED: c.json"));
        let clean = vec![("a.json", PushOutcome::Pushed)];
        assert!(!summarize(&clean).contains("FAILED"));
    }
}
