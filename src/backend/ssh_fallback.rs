//! SSH-FALLBACK mode: when the remote host can't run Code Puppy (no uv /
//! python / PyPI route), the sidecar runs LOCALLY and the workspace still
//! points at the remote project.
//!
//! Honest capability split (see PARITY for the matrix):
//! * GUI surfaces — file tree, editor, git view — use [`SshFs`]/[`SshGit`]
//!   here: every operation is a one-shot `ssh` exec against the real
//!   remote files. The terminal already speaks ssh (B13.7).
//! * The AGENT's own tools are the local code_puppy's: its shell tool can
//!   reach the project via `ssh ... '...'`, but its file read/write/edit
//!   tools touch LOCAL paths only. We do not pretend otherwise — the agent
//!   is told, via a generated AGENTS.md in its scratch cwd (code_puppy
//!   loads `./AGENTS.md` into the system prompt at agent build), to do all
//!   project work through ssh commands. Instruction-following quality is
//!   model-dependent; the UI labels the mode and notes the limits.
//!
//! Latency note: each fs op is an ssh round-trip. The tree wraps this in
//! `CachedFs` (TTL) like the local path; editor open/save are one-shots.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::git::GitRunner;
use crate::workspace::fs::{DirEntry, WorkspaceFs};

use super::ssh::SshTarget;

// ---------------------------------------------------------------------------
// One-shot ssh exec helpers (shared by fs + git).
// ---------------------------------------------------------------------------

/// Run an ssh command to completion, feeding `stdin_bytes` when given.
/// Returns (exit_ok, stdout, stderr). Never logs payload contents.
fn run(mut cmd: Command, stdin_bytes: Option<&[u8]>) -> (bool, String, String) {
    crate::proc::hide_console(&mut cmd);
    cmd.stdin(if stdin_bytes.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, String::new(), format!("starting ssh: {e}")),
    };
    if let (Some(bytes), Some(mut si)) = (stdin_bytes, child.stdin.take()) {
        let _ = si.write_all(bytes);
        // drop -> EOF
    }
    match child.wait_with_output() {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ),
        Err(e) => (false, String::new(), format!("waiting on ssh: {e}")),
    }
}

fn io_err(stderr: &str, what: &str) -> std::io::Error {
    let msg = stderr.trim();
    std::io::Error::other(if msg.is_empty() {
        format!("{what} failed over ssh")
    } else {
        format!("{what}: {msg}")
    })
}

// ---------------------------------------------------------------------------
// SshFs: WorkspaceFs over one-shot ssh execs.
// ---------------------------------------------------------------------------

/// Remote filesystem for SSH-fallback workspaces (tree + editor): every op
/// is `ssh target -- <cmd>`. Paths are the REMOTE absolute paths the
/// workspace root produces.
pub struct SshFs {
    target: SshTarget,
}

impl SshFs {
    pub fn new(target: SshTarget) -> Self {
        SshFs { target }
    }

    fn exec(&self, argv: &[&str], stdin: Option<&[u8]>) -> (bool, String, String) {
        run(self.target.exec_command(argv), stdin)
    }
}

/// A remote `Path` as the str ssh needs (lossy is fine: paths came from the
/// remote as UTF-8 in the first place). The remote is POSIX, but a Windows
/// CLIENT's `PathBuf::join` stitches components with '\\' — which the remote
/// shell would take literally and ENOENT on. Normalize to '/' so a Windows
/// host can drive a Linux remote (backslashes in remote names are vanishingly
/// rare and not worth preserving against this).
fn p(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl WorkspaceFs for SshFs {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        let (ok, out, err) = self.exec(&["cat", &p(path)], None);
        if ok {
            Ok(out)
        } else {
            Err(io_err(&err, "read"))
        }
    }

    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        // `cat > file` over stdin: same convention as sidecar provisioning.
        // `command` defeats rc-file function shadowing (the vm840 lesson).
        let line = format!("command cat > {}", sh_quote_path(path));
        let (ok, _, err) = run(self.target.exec_shell(&line), Some(contents));
        if ok {
            Ok(())
        } else {
            Err(io_err(&err, "write"))
        }
    }

    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntry>> {
        // `ls -1ApL`: dirs carry a trailing '/', dotfiles included. -L
        // dereferences symlinks so a symlink-TO-dir also gets the '/' and is
        // browsable (without -L, `ls -p` leaves symlinked dirs unmarked, so
        // they're mistaken for files and ENOENT on open). A dangling symlink
        // makes ls exit non-zero while still printing the good entries, so we
        // only treat it as failure when nothing came back.
        let (ok, out, err) = self.exec(&["ls", "-1ApL", &p(path)], None);
        if !ok && out.trim().is_empty() {
            return Err(io_err(&err, "list"));
        }
        Ok(parse_ls(path, &out))
    }

    fn create_dir(&self, path: &Path) -> std::io::Result<()> {
        let (ok, _, err) = self.exec(&["mkdir", "-p", &p(path)], None);
        if ok {
            Ok(())
        } else {
            Err(io_err(&err, "mkdir"))
        }
    }

    fn create_file(&self, path: &Path) -> std::io::Result<()> {
        self.write(path, b"")
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        let (ok, _, err) = self.exec(&["rm", "--", &p(path)], None);
        if ok {
            Ok(())
        } else {
            Err(io_err(&err, "remove"))
        }
    }

    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
        let (ok, _, err) = self.exec(&["rm", "-rf", "--", &p(path)], None);
        if ok {
            Ok(())
        } else {
            Err(io_err(&err, "remove"))
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let (ok, _, err) = self.exec(&["mv", "--", &p(from), &p(to)], None);
        if ok {
            Ok(())
        } else {
            Err(io_err(&err, "rename"))
        }
    }

    fn exists(&self, path: &Path) -> bool {
        self.exec(&["test", "-e", &p(path)], None).0
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.exec(&["test", "-d", &p(path)], None).0
    }
}

/// Single-quote a path for an inline remote shell line (write path only;
/// everything else rides `exec_command`'s word quoting).
fn sh_quote_path(path: &Path) -> String {
    let s = p(path);
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

/// Parse `ls -1Ap` output into entries under `base`.
fn parse_ls(base: &Path, out: &str) -> Vec<DirEntry> {
    out.lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                return None;
            }
            let (name, is_dir) = match line.strip_suffix('/') {
                Some(n) => (n, true),
                None => (line, false),
            };
            Some(DirEntry {
                name: name.to_string(),
                path: base.join(name),
                is_dir,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SshGit: GitRunner over one-shot ssh execs; impl_workspace_git! does the
// parsing exactly like the Local/Remote runners.
// ---------------------------------------------------------------------------

/// Runs `git -C <root> ...` on the remote over ssh.
pub struct SshRunner {
    target: SshTarget,
    /// Repo root path on the remote.
    root: String,
}

impl SshRunner {
    fn git(&self, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
        // `env K=V ... git -C root args...` — env runs the command with the
        // vars set; everything is word-quoted by exec_command.
        let mut argv: Vec<String> = Vec::new();
        if !env.is_empty() {
            argv.push("env".into());
            for (k, v) in env {
                argv.push(format!("{k}={v}"));
            }
        }
        argv.extend(["git".to_string(), "-C".to_string(), self.root.clone()]);
        argv.extend(args.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        run(self.target.exec_command(&refs), None)
    }
}

impl GitRunner for SshRunner {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
        let (ok, out, err) = self.git(args, env);
        if ok {
            Ok(out)
        } else {
            let err = err.trim();
            Err(if err.is_empty() {
                format!("git {} failed", args.first().copied().unwrap_or(""))
            } else {
                err.to_string()
            })
        }
    }

    fn output(&self, args: &[&str]) -> String {
        let (ok, out, _) = self.git(args, &[]);
        if ok { out } else { String::new() }
    }

    fn read_workfile(&self, rel: &str) -> Option<String> {
        let base = self.root.trim_end_matches('/');
        let full = if base.is_empty() {
            rel.to_string()
        } else {
            format!("{base}/{rel}")
        };
        let (ok, out, _) = run(self.target.exec_command(&["cat", &full]), None);
        ok.then_some(out)
    }
}

/// Git for SSH-fallback workspaces: git runs on the remote over ssh.
pub struct SshGit {
    runner: SshRunner,
}

impl SshGit {
    pub fn new(target: SshTarget, root: String) -> Self {
        SshGit {
            runner: SshRunner { target, root },
        }
    }
}

crate::git::impl_workspace_git!(SshGit);

// ---------------------------------------------------------------------------
// The local sidecar's scratch cwd + generated AGENTS.md instructions.
// ---------------------------------------------------------------------------

/// Create (or refresh) the scratch dir the LOCAL sidecar runs in, writing
/// an AGENTS.md that instructs the agent to operate on the remote project
/// over ssh. code_puppy loads `./AGENTS.md` from its cwd into the system
/// prompt at agent build — no sidecar/protocol changes needed.
pub fn prepare_scratch_dir(target: &SshTarget, remote_root: &str) -> Result<PathBuf, String> {
    let base = dirs_cache_dir().join("puppy-home").join("ssh-fallback");
    let slug: String = format!("{}-{}", target.destination(), remote_root)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = base.join(slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating scratch dir: {e}"))?;
    std::fs::write(
        dir.join("AGENTS.md"),
        fallback_instructions(&target.destination(), remote_root),
    )
    .map_err(|e| format!("writing AGENTS.md: {e}"))?;
    Ok(dir)
}

fn dirs_cache_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".cache")
}

/// The injected instruction text (kept honest: names the exact limitation).
fn fallback_instructions(dest: &str, root: &str) -> String {
    format!(
        "# SSH-FALLBACK SESSION\n\
         \n\
         You are running on the user's LOCAL machine, but the project you\n\
         are working on lives on a REMOTE host that cannot run you:\n\
         \n\
         - remote: `{dest}`\n\
         - project root: `{root}`\n\
         \n\
         RULES — follow these for EVERY operation:\n\
         1. Perform ALL file and shell operations on the remote via the\n\
            shell tool: `ssh {dest} 'cd {root} && <command>'`.\n\
         2. NEVER use your local file read/write/edit tools for project\n\
            files — local paths are NOT the project. Read files with\n\
            `ssh {dest} 'cat <path>'`; write via heredoc or\n\
            `ssh {dest} 'cat > <path>'` piping.\n\
         3. Your current working directory is a local scratch dir; it is\n\
            fine for temporary notes, never for project files.\n\
         4. ssh is non-interactive (BatchMode): never start editors,\n\
            pagers, or anything expecting a tty; add flags like\n\
            `--no-pager` / `-y` where needed.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ls_marks_dirs_and_joins_paths() {
        let base = Path::new("/srv/proj");
        let entries = parse_ls(base, "src/\nREADME.md\n.git/\n\n");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "src");
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].path, Path::new("/srv/proj/src"));
        assert_eq!(entries[1].name, "README.md");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[2].name, ".git");
        assert!(entries[2].is_dir);
    }

    #[test]
    fn instructions_name_target_root_and_the_limitation() {
        let text = fallback_instructions("alice@devbox", "/srv/proj");
        assert!(text.contains("ssh alice@devbox 'cd /srv/proj"));
        assert!(text.contains("NEVER use your local file read/write/edit tools"));
        assert!(text.contains("SSH-FALLBACK"));
    }

    /// LIVE E2E against the user's vm840 box (the editor/tree/git code
    /// paths verbatim). `#[ignore]` — run explicitly:
    /// `cargo test ssh_fallback_live -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs ssh access to vm840"]
    fn ssh_fallback_live_vm840() {
        let target = SshTarget::parse("vm840").unwrap();
        let root = Path::new("/storage/weinsteinjcc.yobo.dev");
        let fs = SshFs::new(target.clone());

        // Tree: listing matches a few known entries, dirs flagged.
        let entries = fs.read_dir(root).expect("read_dir");
        let find = |n: &str| entries.iter().find(|e| e.name == n);
        assert!(find("README.md").is_some_and(|e| !e.is_dir));
        assert!(find("wp-content").is_some_and(|e| e.is_dir));
        assert!(find(".git").is_some_and(|e| e.is_dir));

        // Editor read: contents match `ssh cat` ground truth.
        let via_fs = fs.read_to_string(&root.join("README.md")).expect("read");
        let (ok, via_ssh, _) = fs.exec(&["cat", "/storage/weinsteinjcc.yobo.dev/README.md"], None);
        assert!(ok);
        assert_eq!(via_fs, via_ssh);

        // Editor write/rename/delete round-trip on a scratch file; the
        // remote is left CLEAN.
        let scratch = root.join(".puppy-home-e2e-scratch.txt");
        let moved = root.join(".puppy-home-e2e-scratch-moved.txt");
        fs.write(&scratch, b"woof woof e2e\n").expect("write");
        assert_eq!(
            fs.read_to_string(&scratch).expect("read back"),
            "woof woof e2e\n"
        );
        fs.rename(&scratch, &moved).expect("rename");
        assert!(!fs.exists(&scratch));
        assert!(fs.exists(&moved));
        fs.remove_file(&moved).expect("remove");
        assert!(!fs.exists(&moved));

        // Git: the path is a real repo on branch `root` with changes.
        let git = SshGit::new(target, root.to_string_lossy().into_owned());
        use crate::git::WorkspaceGit;
        assert!(git.is_repo());
        let status = git.status();
        assert!(!status.is_empty(), "repo had local changes at test time");
    }

    #[test]
    fn scratch_slug_is_filesystem_safe() {
        // Indirect: the slug logic strips anything non-alphanumeric.
        let slug: String = "alice@devbox:22-/srv/my proj"
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        assert_eq!(slug, "alice-devbox-22--srv-my-proj");
    }
}
