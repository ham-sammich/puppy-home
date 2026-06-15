//! Git access for a workspace.
//!
//! Code Puppy has no git API, so we drive git ourselves. The command *parsing*
//! lives in the free functions below, which take a [`GitRunner`] -- an
//! abstraction over "run these git args, give me stdout". `LocalGit` shells out
//! to `git -C <root>`; `RemoteGit` (in the backend) runs git on the SSH host via
//! the sidecar. Both reuse the same porcelain parsing, so behaviour matches.

use std::path::PathBuf;

mod runner;
pub(crate) use runner::{GitRunner, LocalRunner};

/// A working-tree change for one path.
#[derive(Clone)]
pub struct GitChange {
    /// Path relative to the workspace root (forward slashes).
    pub path: String,
    /// 'A' added/new, 'M' modified, 'D' deleted, 'R' renamed, '?' untracked.
    pub marker: char,
}

// ---------------------------------------------------------------------------
// Parsing (shared by local + remote via the runner).
// ---------------------------------------------------------------------------

/// Is the runner's repo inside a git work tree?
pub fn is_repo(r: &dyn GitRunner) -> bool {
    r.run(&["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

/// Working-tree changes (staged + unstaged + untracked), most-relevant first.
pub fn status(r: &dyn GitRunner) -> Vec<GitChange> {
    let text = match r.run(&["status", "--porcelain", "--untracked-files=all"]) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut changes = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let rest = &line[3..];
        // Renames are "old -> new"; keep the new path.
        let path = rest
            .rsplit(" -> ")
            .next()
            .unwrap_or(rest)
            .trim_matches('"')
            .to_string();
        let marker = if xy == "??" {
            '?'
        } else if xy.contains('D') {
            'D'
        } else if xy.contains('A') {
            'A'
        } else if xy.contains('R') {
            'R'
        } else {
            'M'
        };
        changes.push(GitChange { path, marker });
    }
    changes
}

/// Unified diff for one path vs HEAD (covers staged + unstaged). Empty for an
/// untracked file (use [`untracked_content`] for those).
pub fn diff(r: &dyn GitRunner, path: &str) -> String {
    r.output(&["diff", "HEAD", "--", path])
}

/// Read an untracked file's content (rendered as all-added in the diff view).
pub fn untracked_content(r: &dyn GitRunner, path: &str) -> Option<String> {
    r.read_workfile(path)
}

/// Current branch + upstream tracking position.
pub struct RepoInfo {
    pub branch: String,
    pub upstream: bool,
    pub ahead: usize,
    pub behind: usize,
}

/// Branch name and ahead/behind vs the upstream (if any).
pub fn head_info(r: &dyn GitRunner) -> RepoInfo {
    let branch = r
        .run(&["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".to_string());
    // "<behind>\t<ahead>": left = upstream-only commits, right = HEAD-only.
    let (upstream, behind, ahead) =
        match r.run(&["rev-list", "--left-right", "--count", "@{u}...HEAD"]) {
            Ok(s) => {
                let mut it = s.split_whitespace();
                let behind = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let ahead = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                (true, behind, ahead)
            }
            Err(_) => (false, 0, 0),
        };
    RepoInfo {
        branch,
        upstream,
        ahead,
        behind,
    }
}

/// One commit in the history list.
#[derive(Clone, Default)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub when: String,
    pub subject: String,
    /// Full parent hashes (1 for a normal commit, 2+ for a merge, 0 for a root).
    pub parents: Vec<String>,
    /// Decorations on this commit: branch/tag names (e.g. `main`, `tag: v1`).
    pub refs: Vec<String>,
}

/// Recent commits on HEAD (newest first), at most `limit`.
pub fn log(r: &dyn GitRunner, limit: usize) -> Vec<Commit> {
    // %x1f = unit separator between fields; %s never contains a newline.
    let fmt = "--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s";
    let n = format!("-n{limit}");
    let out = match r.run(&["log", &n, fmt]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    out.lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            Some(Commit {
                hash: f.next()?.to_string(),
                short: f.next()?.to_string(),
                author: f.next()?.to_string(),
                when: f.next()?.to_string(),
                subject: f.next().unwrap_or("").to_string(),
                parents: Vec::new(),
                refs: Vec::new(),
            })
        })
        .collect()
}

/// Commits across **all** branches (newest first, date-ordered), with parent
/// hashes + ref decorations — everything the graph view needs to lay out lanes.
pub fn graph_log(r: &dyn GitRunner, limit: usize) -> Vec<Commit> {
    // Fields, %x1f-separated: hash, short, parents, author, age, refs, subject.
    let fmt = "--pretty=format:%H%x1f%h%x1f%P%x1f%an%x1f%ar%x1f%D%x1f%s";
    let n = format!("-n{limit}");
    let out = match r.run(&["log", "--all", "--date-order", &n, fmt]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    out.lines()
        .filter_map(|line| {
            let mut f = line.split('\u{1f}');
            let hash = f.next()?.to_string();
            let short = f.next()?.to_string();
            let parents = f.next()?.split_whitespace().map(str::to_string).collect();
            let author = f.next()?.to_string();
            let when = f.next()?.to_string();
            let refs = parse_refs(f.next().unwrap_or(""));
            let subject = f.next().unwrap_or("").to_string();
            Some(Commit {
                hash,
                short,
                author,
                when,
                subject,
                parents,
                refs,
            })
        })
        .collect()
}

/// Parse git's `%D` decoration string into clean ref labels.
/// Drops the `HEAD -> ` arrow and `tag: ` prefix, keeps the names.
fn parse_refs(d: &str) -> Vec<String> {
    d.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.strip_prefix("HEAD -> ")
                .or_else(|| s.strip_prefix("tag: "))
                .unwrap_or(s)
                .to_string()
        })
        .collect()
}

/// The full patch for a single commit (`git show`), for the diff view.
pub fn show(r: &dyn GitRunner, hash: &str) -> String {
    r.run(&["show", "--stat", "--patch", hash])
        .unwrap_or_default()
}

/// One file's status, split into its index (staged) and worktree (unstaged) sides.
#[derive(Clone)]
pub struct GitStatusEntry {
    pub path: String,
    pub index: char,
    pub worktree: char,
}

impl GitStatusEntry {
    /// Has staged content (something in the index different from HEAD).
    pub fn is_staged(&self) -> bool {
        self.index != ' ' && self.index != '?'
    }
    /// Has unstaged worktree content (modified/deleted/untracked but not staged).
    pub fn is_unstaged(&self) -> bool {
        self.worktree != ' '
    }
    /// Display marker for the worktree-or-index side.
    pub fn marker(&self) -> char {
        marker_for(self.index, self.worktree)
    }
}

fn marker_for(index: char, worktree: char) -> char {
    if index == '?' || worktree == '?' {
        '?'
    } else if index == 'D' || worktree == 'D' {
        'D'
    } else if index == 'A' || worktree == 'A' {
        'A'
    } else if index == 'R' || worktree == 'R' {
        'R'
    } else {
        'M'
    }
}

/// Full porcelain status with index/worktree sides preserved.
pub fn status_full(r: &dyn GitRunner) -> Vec<GitStatusEntry> {
    let out = match r.run(&["status", "--porcelain", "--untracked-files=all"]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let mut entries = Vec::new();
    for line in out.lines() {
        if line.len() < 4 {
            continue;
        }
        let index = line.chars().next().unwrap_or(' ');
        let worktree = line.chars().nth(1).unwrap_or(' ');
        let rest = &line[3..];
        let path = rest
            .rsplit(" -> ")
            .next()
            .unwrap_or(rest)
            .trim_matches('"')
            .to_string();
        entries.push(GitStatusEntry {
            path,
            index,
            worktree,
        });
    }
    entries
}

/// Stage one path (`git add`).
pub fn stage(r: &dyn GitRunner, path: &str) -> Result<(), String> {
    r.run(&["add", "--", path]).map(|_| ())
}

/// Unstage one path (`git restore --staged`).
pub fn unstage(r: &dyn GitRunner, path: &str) -> Result<(), String> {
    r.run(&["restore", "--staged", "--", path]).map(|_| ())
}

/// Stage everything (`git add -A`).
pub fn stage_all(r: &dyn GitRunner) -> Result<(), String> {
    r.run(&["add", "-A"]).map(|_| ())
}

/// Unstage everything (`git reset`).
pub fn unstage_all(r: &dyn GitRunner) -> Result<(), String> {
    r.run(&["reset", "-q"]).map(|_| ())
}

/// Commit the staged changes. Returns the short summary git prints, or an error.
pub fn commit(r: &dyn GitRunner, message: &str) -> Result<String, String> {
    r.run(&["commit", "-m", message])
        .map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// Graph actions: branch/checkout/cherry-pick/revert/reset + remote sync.
// ---------------------------------------------------------------------------

/// Switch to an existing branch (or detach onto a tag/remote ref).
pub fn checkout(r: &dyn GitRunner, name: &str) -> Result<(), String> {
    r.run(&["checkout", name]).map(|_| ())
}

/// Create a new branch at `at` and switch to it.
pub fn create_branch(r: &dyn GitRunner, name: &str, at: &str) -> Result<(), String> {
    r.run(&["checkout", "-b", name, at]).map(|_| ())
}

/// Merge `target` (a branch/commit) into the current branch.
pub fn merge(r: &dyn GitRunner, target: &str) -> Result<String, String> {
    r.run(&["merge", target]).map(|s| s.trim().to_string())
}

/// Apply a commit onto the current branch.
pub fn cherry_pick(r: &dyn GitRunner, hash: &str) -> Result<(), String> {
    r.run(&["cherry-pick", hash]).map(|_| ())
}

/// Create a new commit that undoes `hash` (no editor).
pub fn revert(r: &dyn GitRunner, hash: &str) -> Result<(), String> {
    r.run(&["revert", "--no-edit", hash]).map(|_| ())
}

/// Move the current branch tip to `hash`. `mode` is a git reset flag such as
/// `--soft`, `--mixed`, or `--hard` (the last discards working-tree changes).
pub fn reset(r: &dyn GitRunner, hash: &str, mode: &str) -> Result<(), String> {
    r.run(&["reset", mode, hash]).map(|_| ())
}

/// Fetch all remotes and prune stale tracking branches.
pub fn fetch(r: &dyn GitRunner) -> Result<String, String> {
    r.run(&["fetch", "--all", "--prune"])
        .map(|s| s.trim().to_string())
}

/// Fast-forward the current branch from its upstream.
pub fn pull(r: &dyn GitRunner) -> Result<String, String> {
    r.run(&["pull", "--ff-only"]).map(|s| s.trim().to_string())
}

/// Push the current branch to its upstream.
pub fn push(r: &dyn GitRunner) -> Result<String, String> {
    r.run(&["push"]).map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// Authenticated network ops: HTTPS remotes that want a username/password (or
// PAT). When a plain push/pull/fetch fails for auth, the GUI collects creds and
// retries via these, which feed them to git through a one-shot credential
// helper reading `$GIT_USER`/`$GIT_PASS` from the env -- nothing is persisted,
// and the password never appears in argv/`ps`.
// ---------------------------------------------------------------------------

/// Does a git error indicate it needs an HTTPS username/password? (Distinct
/// from an SSH key problem like "Permission denied (publickey)".)
pub fn is_auth_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("could not read username")
        || m.contains("could not read password")
        || m.contains("authentication failed")
        || m.contains("terminal prompts disabled")
        || m.contains("invalid username or password")
}

/// A `credential.helper` that echoes `$GIT_USER`/`$GIT_PASS` on a `get`.
const CRED_HELPER: &str = "credential.helper=!f() { test \"$1\" = get && \
printf 'username=%s\\npassword=%s\\n' \"$GIT_USER\" \"$GIT_PASS\"; }; f";

/// Run a network subcommand with credentials injected via the helper + env.
/// The leading empty `credential.helper=` clears any system helpers first, so
/// ours is the only one consulted.
fn run_with_creds(
    r: &dyn GitRunner,
    subcmd: &[&str],
    user: &str,
    pass: &str,
) -> Result<String, String> {
    let mut args: Vec<&str> = vec!["-c", "credential.helper=", "-c", CRED_HELPER];
    args.extend_from_slice(subcmd);
    r.run_env(&args, &[("GIT_USER", user), ("GIT_PASS", pass)])
        .map(|s| s.trim().to_string())
}

/// Push with explicit credentials.
pub fn push_auth(r: &dyn GitRunner, user: &str, pass: &str) -> Result<String, String> {
    run_with_creds(r, &["push"], user, pass)
}

/// Pull (fast-forward only) with explicit credentials.
pub fn pull_auth(r: &dyn GitRunner, user: &str, pass: &str) -> Result<String, String> {
    run_with_creds(r, &["pull", "--ff-only"], user, pass)
}

/// Fetch all remotes with explicit credentials.
pub fn fetch_auth(r: &dyn GitRunner, user: &str, pass: &str) -> Result<String, String> {
    run_with_creds(r, &["fetch", "--all", "--prune"], user, pass)
}

/// One blamed line: the commit that last touched it + the content.
pub struct BlameLine {
    pub short: String,
    pub author: String,
    pub date: String,
    pub line: String,
}

/// Per-line blame for a file (`git blame --date=short`).
pub fn blame(r: &dyn GitRunner, path: &str) -> Vec<BlameLine> {
    let out = match r.run(&["blame", "--date=short", "--", path]) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let mut lines = Vec::new();
    for raw in out.lines() {
        // Format: `<hash> (<author> <YYYY-MM-DD> <lineno>) <content>`
        let (Some(lp), Some(rp)) = (raw.find('('), raw.find(')')) else {
            continue;
        };
        if rp < lp {
            continue;
        }
        let hash = raw[..lp].trim().trim_start_matches('^');
        let short: String = hash.chars().take(8).collect();
        let inside = &raw[lp + 1..rp];
        let mut toks: Vec<&str> = inside.split_whitespace().collect();
        let _lineno = toks.pop();
        let date = toks.pop().unwrap_or("").to_string();
        let author = toks.join(" ");
        let content = raw[rp + 1..].strip_prefix(' ').unwrap_or(&raw[rp + 1..]);
        lines.push(BlameLine {
            short,
            author,
            date,
            line: content.to_string(),
        });
    }
    lines
}

// ---------------------------------------------------------------------------
// Trait wrapper: a workspace holds a git backend (local or remote) it drives as
// `self.git.<op>()`. Both impls delegate to the free functions above through a
// `GitRunner`, so the parsing is shared.
// ---------------------------------------------------------------------------

/// A workspace's git backend, with the repo + transport captured.
pub trait WorkspaceGit: Send + Sync {
    fn is_repo(&self) -> bool;
    fn status(&self) -> Vec<GitChange>;
    fn diff(&self, path: &str) -> String;
    fn untracked_content(&self, path: &str) -> Option<String>;
    fn head_info(&self) -> RepoInfo;
    fn log(&self, limit: usize) -> Vec<Commit>;
    fn graph_log(&self, limit: usize) -> Vec<Commit>;
    fn show(&self, hash: &str) -> String;
    fn status_full(&self) -> Vec<GitStatusEntry>;
    fn stage(&self, path: &str) -> Result<(), String>;
    fn unstage(&self, path: &str) -> Result<(), String>;
    fn stage_all(&self) -> Result<(), String>;
    fn unstage_all(&self) -> Result<(), String>;
    fn commit(&self, message: &str) -> Result<String, String>;
    fn checkout(&self, name: &str) -> Result<(), String>;
    fn create_branch(&self, name: &str, at: &str) -> Result<(), String>;
    fn merge(&self, target: &str) -> Result<String, String>;
    fn cherry_pick(&self, hash: &str) -> Result<(), String>;
    fn revert(&self, hash: &str) -> Result<(), String>;
    fn reset(&self, hash: &str, mode: &str) -> Result<(), String>;
    fn fetch(&self) -> Result<String, String>;
    fn pull(&self) -> Result<String, String>;
    fn push(&self) -> Result<String, String>;
    /// Retry fetch/pull/push with an explicit HTTPS username + password/token.
    fn fetch_auth(&self, user: &str, pass: &str) -> Result<String, String>;
    fn pull_auth(&self, user: &str, pass: &str) -> Result<String, String>;
    fn push_auth(&self, user: &str, pass: &str) -> Result<String, String>;
    fn blame(&self, path: &str) -> Vec<BlameLine>;
}

/// Generate a `WorkspaceGit` impl that delegates to the free functions through
/// `self.runner` (a [`GitRunner`]). Local + remote backends differ only in the
/// runner, so the whole impl is identical.
macro_rules! impl_workspace_git {
    ($ty:ty) => {
        #[rustfmt::skip]
        impl $crate::git::WorkspaceGit for $ty {
            fn is_repo(&self) -> bool { $crate::git::is_repo(&self.runner) }
            fn status(&self) -> Vec<$crate::git::GitChange> { $crate::git::status(&self.runner) }
            fn diff(&self, path: &str) -> String { $crate::git::diff(&self.runner, path) }
            fn untracked_content(&self, path: &str) -> Option<String> { $crate::git::untracked_content(&self.runner, path) }
            fn head_info(&self) -> $crate::git::RepoInfo { $crate::git::head_info(&self.runner) }
            fn log(&self, limit: usize) -> Vec<$crate::git::Commit> { $crate::git::log(&self.runner, limit) }
            fn graph_log(&self, limit: usize) -> Vec<$crate::git::Commit> { $crate::git::graph_log(&self.runner, limit) }
            fn show(&self, hash: &str) -> String { $crate::git::show(&self.runner, hash) }
            fn status_full(&self) -> Vec<$crate::git::GitStatusEntry> { $crate::git::status_full(&self.runner) }
            fn stage(&self, path: &str) -> Result<(), String> { $crate::git::stage(&self.runner, path) }
            fn unstage(&self, path: &str) -> Result<(), String> { $crate::git::unstage(&self.runner, path) }
            fn stage_all(&self) -> Result<(), String> { $crate::git::stage_all(&self.runner) }
            fn unstage_all(&self) -> Result<(), String> { $crate::git::unstage_all(&self.runner) }
            fn commit(&self, message: &str) -> Result<String, String> { $crate::git::commit(&self.runner, message) }
            fn checkout(&self, name: &str) -> Result<(), String> { $crate::git::checkout(&self.runner, name) }
            fn create_branch(&self, name: &str, at: &str) -> Result<(), String> { $crate::git::create_branch(&self.runner, name, at) }
            fn merge(&self, target: &str) -> Result<String, String> { $crate::git::merge(&self.runner, target) }
            fn cherry_pick(&self, hash: &str) -> Result<(), String> { $crate::git::cherry_pick(&self.runner, hash) }
            fn revert(&self, hash: &str) -> Result<(), String> { $crate::git::revert(&self.runner, hash) }
            fn reset(&self, hash: &str, mode: &str) -> Result<(), String> { $crate::git::reset(&self.runner, hash, mode) }
            fn fetch(&self) -> Result<String, String> { $crate::git::fetch(&self.runner) }
            fn pull(&self) -> Result<String, String> { $crate::git::pull(&self.runner) }
            fn push(&self) -> Result<String, String> { $crate::git::push(&self.runner) }
            fn fetch_auth(&self, user: &str, pass: &str) -> Result<String, String> { $crate::git::fetch_auth(&self.runner, user, pass) }
            fn pull_auth(&self, user: &str, pass: &str) -> Result<String, String> { $crate::git::pull_auth(&self.runner, user, pass) }
            fn push_auth(&self, user: &str, pass: &str) -> Result<String, String> { $crate::git::push_auth(&self.runner, user, pass) }
            fn blame(&self, path: &str) -> Vec<$crate::git::BlameLine> { $crate::git::blame(&self.runner, path) }
        }
    };
}

pub(crate) use impl_workspace_git;

/// The local-git backend: shells out to `git -C <root>`.
pub struct LocalGit {
    runner: LocalRunner,
}

impl LocalGit {
    pub fn new(root: PathBuf) -> Self {
        LocalGit {
            runner: LocalRunner::new(root),
        }
    }
}

impl_workspace_git!(LocalGit);
