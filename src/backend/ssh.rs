//! System-`ssh` transport for running the sidecar on a remote host.
//!
//! Phase A spike (increment 3). We shell out to the user's own `ssh` binary so
//! their existing `~/.ssh/config`, keys, and agent Just Work -- puppy-home does
//! no auth of its own. Bringing up a remote sidecar is two `ssh` calls:
//!
//!   1. **provision** -- ship the embedded `sidecar.py` to a fixed cache path on
//!      the remote: `ssh host -- 'mkdir -p ... && cat > .../sidecar.py'`, feeding
//!      the bytes on stdin. (sidecar.py is ~71 KB, too big for an argv.)
//!   2. **launch** -- `ssh -T host -- cd <cwd> && exec <launcher> .../sidecar.py`,
//!      whose stdin/stdout then carry the *same* newline-JSON protocol the local
//!      sidecar uses, so the rest of the backend is transport-agnostic.
//!
//! This module is pure command construction (no processes), so it is unit
//! tested offline; `CodePuppy::spawn_remote` in the parent module runs the two
//! commands and reuses the existing stdout/stderr reader plumbing.
//!
//! Auth note: we pass `BatchMode=yes` (fail fast instead of hanging when no key
//! works -- the GUI has no tty to type a password into) and
//! `StrictHostKeyChecking=accept-new` (trust-on-first-use, still reject *changed*
//! keys). A future increment can expose these as per-profile toggles.

use std::path::PathBuf;
use std::process::Command;

/// Where the sidecar is cached on the remote. Uses `$HOME` (expanded by the
/// remote shell), so it must stay double-quoted / unquoted in commands -- never
/// single-quoted.
const REMOTE_DIR: &str = "\"$HOME/.cache/puppy-home\"";
const REMOTE_SIDECAR: &str = "\"$HOME/.cache/puppy-home/sidecar.py\"";

/// The default remote launcher (mirrors the local `uv` auto-provision path).
/// Overridable via `PUPPY_HOME_REMOTE_CP_CMD` (whitespace-split; the sidecar
/// path is appended after it).
const DEFAULT_REMOTE_LAUNCHER: &str = "uv run --with code-puppy python";

/// A remote SSH destination plus the knobs we pass to `ssh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    /// Hostname or a `~/.ssh/config` alias.
    pub host: String,
    /// Login user (omitted -> ssh/config decides).
    pub user: Option<String>,
    /// Port (omitted -> 22 / config).
    pub port: Option<u16>,
    /// Explicit identity (private key) file.
    pub identity: Option<PathBuf>,
}

impl SshTarget {
    /// Parse a `[user@]host[:port]` string. `host` may be a config alias.
    ///
    /// Not yet called outside tests (connection-profile UI lands in a later
    /// increment), so it is allowed to be unused for now.
    #[allow(dead_code)]
    pub fn parse(spec: &str) -> Result<SshTarget, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty SSH target".into());
        }
        let (user, rest) = match spec.split_once('@') {
            Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
            Some(_) => return Err("SSH target has an empty user".into()),
            None => (None, spec),
        };
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => {
                let port = p
                    .parse::<u16>()
                    .map_err(|_| format!("invalid SSH port: {p:?}"))?;
                (h, Some(port))
            }
            None => (rest, None),
        };
        if host.is_empty() {
            return Err("SSH target has an empty host".into());
        }
        Ok(SshTarget {
            host: host.to_string(),
            user,
            port,
            identity: None,
        })
    }

    /// `user@host` (or just `host`).
    pub fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// A base `ssh` invocation: common flags + destination, no remote command.
    fn base_ssh(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.arg("-T") // no pseudo-tty; stdio carries the protocol verbatim
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new");
        if let Some(port) = self.port {
            cmd.arg("-p").arg(port.to_string());
        }
        if let Some(id) = &self.identity {
            cmd.arg("-i").arg(id);
        }
        cmd.arg(self.destination());
        cmd
    }

    /// The `ssh` call that ships `sidecar.py` to the remote cache path; feed the
    /// sidecar bytes on its stdin.
    pub fn provision_command(&self) -> Command {
        let mut cmd = self.base_ssh();
        cmd.arg("--")
            .arg(format!("mkdir -p {REMOTE_DIR} && cat > {REMOTE_SIDECAR}"));
        cmd
    }

    /// The `ssh` call that launches the remote sidecar. Its stdin/stdout carry
    /// the JSON protocol. `cwd`, when set, becomes the remote working dir.
    pub fn launch_command(&self, cwd: Option<&str>, launcher: &str) -> Command {
        let mut cmd = self.base_ssh();
        cmd.arg("--").arg(remote_launch_line(cwd, launcher));
        cmd
    }
}

/// The remote shell line that starts the sidecar (used as the `ssh` command).
fn remote_launch_line(cwd: Option<&str>, launcher: &str) -> String {
    let run = format!("exec {launcher} {REMOTE_SIDECAR}");
    match cwd {
        Some(dir) => format!("cd {} && {run}", sh_quote(dir)),
        None => run,
    }
}

/// The default remote launcher, honouring `PUPPY_HOME_REMOTE_CP_CMD`.
pub fn default_remote_launcher() -> String {
    std::env::var("PUPPY_HOME_REMOTE_CP_CMD")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_REMOTE_LAUNCHER.to_string())
}

/// Single-quote a string for a POSIX shell (wraps in `'...'`, escaping any
/// embedded single quotes as `'\''`). Safe for arbitrary paths.
fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect a Command's args as lossy strings (for assertions).
    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn parses_user_host_port() {
        let t = SshTarget::parse("alice@build.example.com:2222").unwrap();
        assert_eq!(t.user.as_deref(), Some("alice"));
        assert_eq!(t.host, "build.example.com");
        assert_eq!(t.port, Some(2222));
        // The port rides as an `-p` flag, not in the destination string.
        assert_eq!(t.destination(), "alice@build.example.com");
    }

    #[test]
    fn parses_bare_host_and_alias() {
        let t = SshTarget::parse("devbox").unwrap();
        assert_eq!(t.user, None);
        assert_eq!(t.host, "devbox");
        assert_eq!(t.port, None);
        assert_eq!(t.destination(), "devbox");
    }

    #[test]
    fn rejects_bad_targets() {
        assert!(SshTarget::parse("").is_err());
        assert!(SshTarget::parse("@host").is_err());
        assert!(SshTarget::parse("host:notaport").is_err());
    }

    #[test]
    fn base_ssh_has_safe_noninteractive_flags() {
        let t = SshTarget::parse("devbox").unwrap();
        let a = args(&t.base_ssh());
        assert!(a.contains(&"-T".to_string()));
        assert!(a.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(
            a.windows(2)
                .any(|w| w == ["-o", "StrictHostKeyChecking=accept-new"])
        );
        // destination is the final arg, no remote command appended.
        assert_eq!(a.last().unwrap(), "devbox");
    }

    #[test]
    fn port_and_identity_become_flags() {
        let mut t = SshTarget::parse("alice@host:2222").unwrap();
        t.identity = Some(PathBuf::from("/keys/id_ed25519"));
        let a = args(&t.base_ssh());
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(a.windows(2).any(|w| w == ["-i", "/keys/id_ed25519"]));
    }

    #[test]
    fn provision_pipes_into_the_cache_path() {
        let t = SshTarget::parse("devbox").unwrap();
        let a = args(&t.provision_command());
        let remote = a.last().unwrap();
        assert!(remote.starts_with("mkdir -p "));
        assert!(remote.contains("cat > \"$HOME/.cache/puppy-home/sidecar.py\""));
    }

    #[test]
    fn launch_line_quotes_cwd_and_uses_launcher() {
        let line = remote_launch_line(Some("/srv/my repo"), "uv run python");
        assert_eq!(
            line,
            "cd '/srv/my repo' && exec uv run python \"$HOME/.cache/puppy-home/sidecar.py\""
        );
    }

    #[test]
    fn launch_line_without_cwd() {
        let line = remote_launch_line(None, "python3");
        assert_eq!(line, "exec python3 \"$HOME/.cache/puppy-home/sidecar.py\"");
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_quote("/a b/c"), "'/a b/c'");
    }
}
