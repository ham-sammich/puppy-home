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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
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

    /// Argv (after `ssh`) for an INTERACTIVE terminal session on this target,
    /// landing in `remote_root` and exec'ing the login shell (the remote
    /// workspace's embedded terminal, B13.7).
    ///
    /// Deliberately NOT `base_ssh` conventions: the terminal runs on a real
    /// PTY, so `-t` allocates a remote tty and there is no `BatchMode=yes` —
    /// password/2FA prompts can flow through the terminal, unlike the
    /// protocol channels which must fail fast instead of hanging.
    /// Host-key and timeout conventions match the sidecar connection.
    pub fn terminal_args(&self, remote_root: &str) -> Vec<String> {
        let mut args = vec![
            "-t".to_string(),
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
        ];
        if let Some(port) = self.port {
            args.push("-p".to_string());
            args.push(port.to_string());
        }
        if let Some(id) = &self.identity {
            args.push("-i".to_string());
            args.push(id.to_string_lossy().into_owned());
        }
        args.push(self.destination());
        args.push("--".to_string());
        args.push(format!(
            "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
            sh_quote(remote_root)
        ));
        args
    }

    /// The `ssh` call that lists a remote directory for the folder picker.
    /// `dir` of `None` means the login home. stdout is [`parse_listing`]-shaped.
    pub fn list_dir_command(&self, dir: Option<&str>) -> Command {
        let mut cmd = self.base_ssh();
        cmd.arg("--").arg(remote_list_line(dir));
        cmd
    }
}

/// The remote shell line that resolves + lists a directory: the first output
/// line is the absolute path (`pwd`), the rest are entries (`ls -1Ap`, so dirs
/// carry a trailing `/`). `dir` of `None` lists the login home.
fn remote_list_line(dir: Option<&str>) -> String {
    match dir {
        Some(d) => format!("cd {} && pwd && ls -1Ap", sh_quote(d)),
        None => "cd && pwd && ls -1Ap".to_string(),
    }
}

/// Parse [`list_dir_command`](SshTarget::list_dir_command) stdout into the
/// resolved absolute directory and its `(name, is_dir)` entries.
pub fn parse_listing(stdout: &str) -> Option<(String, Vec<(String, bool)>)> {
    let mut lines = stdout.lines();
    let cwd = lines.next()?.trim_end_matches('\r').to_string();
    if !cwd.starts_with('/') {
        return None; // `pwd` must be absolute; otherwise the `cd` failed
    }
    let mut entries = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        match line.strip_suffix('/') {
            Some(name) => entries.push((name.to_string(), true)),
            None => entries.push((line.to_string(), false)),
        }
    }
    Some((cwd, entries))
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

// ---------------------------------------------------------------------------
// Host discovery: read connectable aliases from the user's ssh config so the
// connect picker can offer them (alongside free-text entry).
// ---------------------------------------------------------------------------

/// Connectable `Host` aliases from `~/.ssh/config` (following `Include`s),
/// de-duplicated in first-seen order. Wildcard/negated patterns (`Host *`,
/// `web-*`, `!bastion`) are skipped -- they aren't real destinations.
pub fn config_hosts() -> Vec<String> {
    match home_dir() {
        Some(home) => config_hosts_in(&home),
        None => Vec::new(),
    }
}

/// Host discovery rooted at an explicit home dir (so tests need not touch the
/// process-global `HOME`).
fn config_hosts_in(home: &Path) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut seen = HashSet::new();
    collect_config(
        &home.join(".ssh").join("config"),
        home,
        &mut hosts,
        &mut seen,
        0,
    );
    hosts
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Parse one config file into `hosts`, recursing into `Include`d files. `depth`
/// guards against include cycles.
fn collect_config(
    path: &Path,
    home: &Path,
    hosts: &mut Vec<String>,
    seen: &mut HashSet<String>,
    depth: u8,
) {
    if depth > 8 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(keyword) = words.next() else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            for alias in words {
                if is_plain_alias(alias) && seen.insert(alias.to_string()) {
                    hosts.push(alias.to_string());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            for pattern in words {
                for inc in expand_include(pattern, home) {
                    collect_config(&inc, home, hosts, seen, depth + 1);
                }
            }
        }
    }
}

/// A real, connectable alias: no glob/negation metacharacters.
fn is_plain_alias(tok: &str) -> bool {
    !tok.is_empty() && !tok.contains(['*', '?', '!'])
}

/// Resolve an `Include` pattern (relative to `~/.ssh`, `~` expanded) into the
/// files it names, expanding a single-level `*` glob in the last component.
fn expand_include(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let resolved = if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else {
        let p = Path::new(pattern);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            home.join(".ssh").join(p)
        }
    };
    let Some(fname) = resolved.file_name().and_then(|s| s.to_str()) else {
        return vec![resolved];
    };
    if !fname.contains(['*', '?']) {
        return vec![resolved];
    }
    let dir = resolved.parent().unwrap_or_else(|| Path::new("."));
    let mut out = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && glob_match(fname, name)
            {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// Minimal shell-style wildcard match (`*` any run, `?` one char). ASCII paths.
fn glob_match(pattern: &str, name: &str) -> bool {
    let (pb, nb) = (pattern.as_bytes(), name.as_bytes());
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ni < nb.len() {
        if pi < pb.len() && (pb[pi] == b'?' || pb[pi] == nb[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < pb.len() && pb[pi] == b'*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
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
    fn remote_list_line_home_and_dir() {
        assert_eq!(remote_list_line(None), "cd && pwd && ls -1Ap");
        assert_eq!(
            remote_list_line(Some("/srv/my repo")),
            "cd '/srv/my repo' && pwd && ls -1Ap"
        );
    }

    #[test]
    fn parse_listing_splits_pwd_and_marks_dirs() {
        let (cwd, entries) = parse_listing("/home/alice\nsrc/\nREADME.md\n.config/\n\n").unwrap();
        assert_eq!(cwd, "/home/alice");
        assert_eq!(
            entries,
            vec![
                ("src".to_string(), true),
                ("README.md".to_string(), false),
                (".config".to_string(), true),
            ]
        );
        // A non-absolute first line means the remote `cd` failed.
        assert!(parse_listing("no such dir\n").is_none());
        assert!(parse_listing("").is_none());
    }

    #[test]
    fn launch_line_without_cwd() {
        let line = remote_launch_line(None, "python3");
        assert_eq!(line, "exec python3 \"$HOME/.cache/puppy-home/sidecar.py\"");
    }

    #[test]
    fn terminal_args_interactive_shape() {
        let mut t = SshTarget::parse("alice@host:2222").unwrap();
        t.identity = Some(PathBuf::from("/keys/id_ed25519"));
        let a = t.terminal_args("/srv/my repo");
        // Interactive: tty forced, NO BatchMode (the PTY can take a password).
        assert_eq!(a[0], "-t");
        assert!(!a.iter().any(|s| s.contains("BatchMode")));
        // Sidecar-matching conventions.
        assert!(
            a.windows(2)
                .any(|w| w == ["-o", "StrictHostKeyChecking=accept-new"])
        );
        assert!(a.windows(2).any(|w| w == ["-o", "ConnectTimeout=10"]));
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(a.windows(2).any(|w| w == ["-i", "/keys/id_ed25519"]));
        // Destination then the remote line: quoted cd + login-shell exec.
        let di = a.iter().position(|s| s == "alice@host").unwrap();
        assert_eq!(a[di + 1], "--");
        assert_eq!(
            a[di + 2],
            "cd '/srv/my repo' && exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[test]
    fn terminal_args_minimal_target() {
        let a = SshTarget::parse("devbox").unwrap().terminal_args("/srv");
        assert!(!a.iter().any(|s| s == "-p" || s == "-i"));
        assert!(a.contains(&"devbox".to_string()));
        assert_eq!(
            a.last().unwrap(),
            "cd '/srv' && exec \"${SHELL:-/bin/sh}\" -l"
        );
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("plain"), "'plain'");
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_quote("/a b/c"), "'/a b/c'");
    }

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("config.d", "config.d"));
        assert!(glob_match("*.conf", "work.conf"));
        assert!(glob_match("host-*", "host-1"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*.conf", "work.cfg"));
        assert!(!glob_match("a?c", "ac"));
    }

    #[test]
    fn config_hosts_reads_aliases_and_follows_include() {
        let base = std::env::temp_dir().join(format!(
            "ph_ssh_cfg_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let ssh = base.join(".ssh");
        let confd = ssh.join("config.d");
        std::fs::create_dir_all(&confd).unwrap();

        std::fs::write(
            ssh.join("config"),
            "# my hosts\n\
             Host devbox prod\n    HostName 10.0.0.1\n\
             Host *\n    User skip-me\n\
             Host web-*\n    User skip-pattern\n\
             Include config.d/*\n",
        )
        .unwrap();
        std::fs::write(confd.join("extra.conf"), "Host buildbox\n    Port 2222\n").unwrap();

        let hosts = config_hosts_in(&base);
        let _ = std::fs::remove_dir_all(&base);

        assert!(hosts.contains(&"devbox".to_string()));
        assert!(hosts.contains(&"prod".to_string()));
        assert!(
            hosts.contains(&"buildbox".to_string()),
            "Include not followed: {hosts:?}"
        );
        assert!(!hosts.iter().any(|h| h.contains('*')));
    }
}
