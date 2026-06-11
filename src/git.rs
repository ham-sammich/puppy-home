//! Thin, read-only `git` wrapper (Code Puppy has no git APIs, so we shell out).
//!
//! Used to drive the Changes panel from the real working-tree status instead of
//! only the edits Code Puppy reports — so locally-made changes show up too.

use std::path::Path;
use std::process::{Command, Stdio};

/// A working-tree change for one path.
#[derive(Clone)]
pub struct GitChange {
    /// Path relative to the workspace root (forward slashes).
    pub path: String,
    /// 'A' added/new, 'M' modified, 'D' deleted, 'R' renamed, '?' untracked.
    pub marker: char,
}

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    crate::proc::hide_console(&mut c);
    c.arg("-C").arg(root);
    c
}

/// Is `root` inside a git work tree?
pub fn is_repo(root: &Path) -> bool {
    git(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Working-tree changes (staged + unstaged + untracked), most-relevant first.
pub fn status(root: &Path) -> Vec<GitChange> {
    let out = match git(root)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
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
pub fn diff(root: &Path, path: &str) -> String {
    git(root)
        .args(["diff", "HEAD", "--", path])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Read an untracked file's content (rendered as all-added in the diff view).
pub fn untracked_content(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path)).ok()
}

// ---------------------------------------------------------------------------
// Git view: branch, history, staging, commit, blame.
// ---------------------------------------------------------------------------

/// Run a git command, returning stdout on success or a trimmed error on failure.
fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    match git(root).args(args).output() {
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

/// Current branch + upstream tracking position.
pub struct RepoInfo {
    pub branch: String,
    pub upstream: bool,
    pub ahead: usize,
    pub behind: usize,
}

/// Branch name and ahead/behind vs the upstream (if any).
pub fn head_info(root: &Path) -> RepoInfo {
    let branch = run(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "HEAD".to_string());
    // "<behind>\t<ahead>": left = upstream-only commits, right = HEAD-only.
    let (upstream, behind, ahead) = match run(
        root,
        &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
    ) {
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
pub fn log(root: &Path, limit: usize) -> Vec<Commit> {
    // %x1f = unit separator between fields; %s never contains a newline.
    let fmt = "--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s";
    let n = format!("-n{limit}");
    let out = match run(root, &["log", &n, fmt]) {
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
pub fn graph_log(root: &Path, limit: usize) -> Vec<Commit> {
    // Fields, %x1f-separated: hash, short, parents, author, age, refs, subject.
    let fmt = "--pretty=format:%H%x1f%h%x1f%P%x1f%an%x1f%ar%x1f%D%x1f%s";
    let n = format!("-n{limit}");
    let out = match run(root, &["log", "--all", "--date-order", &n, fmt]) {
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
pub fn show(root: &Path, hash: &str) -> String {
    run(root, &["show", "--stat", "--patch", hash]).unwrap_or_default()
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
pub fn status_full(root: &Path) -> Vec<GitStatusEntry> {
    let out = match run(root, &["status", "--porcelain", "--untracked-files=all"]) {
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
pub fn stage(root: &Path, path: &str) -> Result<(), String> {
    run(root, &["add", "--", path]).map(|_| ())
}

/// Unstage one path (`git restore --staged`).
pub fn unstage(root: &Path, path: &str) -> Result<(), String> {
    run(root, &["restore", "--staged", "--", path]).map(|_| ())
}

/// Stage everything (`git add -A`).
pub fn stage_all(root: &Path) -> Result<(), String> {
    run(root, &["add", "-A"]).map(|_| ())
}

/// Unstage everything (`git reset`).
pub fn unstage_all(root: &Path) -> Result<(), String> {
    run(root, &["reset", "-q"]).map(|_| ())
}

/// Commit the staged changes. Returns the short summary git prints, or an error.
pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    run(root, &["commit", "-m", message]).map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// Graph actions: branch/checkout/cherry-pick/revert/reset + remote sync.
// ---------------------------------------------------------------------------

/// Switch to an existing branch (or detach onto a tag/remote ref).
pub fn checkout(root: &Path, name: &str) -> Result<(), String> {
    run(root, &["checkout", name]).map(|_| ())
}

/// Create a new branch at `at` and switch to it.
pub fn create_branch(root: &Path, name: &str, at: &str) -> Result<(), String> {
    run(root, &["checkout", "-b", name, at]).map(|_| ())
}

/// Force-delete a local branch.
pub fn delete_branch(root: &Path, name: &str) -> Result<(), String> {
    run(root, &["branch", "-D", name]).map(|_| ())
}

/// Merge `target` (a branch/commit) into the current branch.
pub fn merge(root: &Path, target: &str) -> Result<String, String> {
    run(root, &["merge", target]).map(|s| s.trim().to_string())
}

/// Apply a commit onto the current branch.
pub fn cherry_pick(root: &Path, hash: &str) -> Result<(), String> {
    run(root, &["cherry-pick", hash]).map(|_| ())
}

/// Create a new commit that undoes `hash` (no editor).
pub fn revert(root: &Path, hash: &str) -> Result<(), String> {
    run(root, &["revert", "--no-edit", hash]).map(|_| ())
}

/// Move the current branch tip to `hash`. `mode` is a git reset flag such as
/// `--soft`, `--mixed`, or `--hard` (the last discards working-tree changes).
pub fn reset(root: &Path, hash: &str, mode: &str) -> Result<(), String> {
    run(root, &["reset", mode, hash]).map(|_| ())
}

/// Fetch all remotes and prune stale tracking branches.
pub fn fetch(root: &Path) -> Result<String, String> {
    run(root, &["fetch", "--all", "--prune"]).map(|s| s.trim().to_string())
}

/// Fast-forward the current branch from its upstream.
pub fn pull(root: &Path) -> Result<String, String> {
    run(root, &["pull", "--ff-only"]).map(|s| s.trim().to_string())
}

/// Push the current branch to its upstream.
pub fn push(root: &Path) -> Result<String, String> {
    run(root, &["push"]).map(|s| s.trim().to_string())
}

/// One blamed line: the commit that last touched it + the content.
pub struct BlameLine {
    pub short: String,
    pub author: String,
    pub date: String,
    pub line: String,
}

/// Per-line blame for a file (`git blame --date=short`).
pub fn blame(root: &Path, path: &str) -> Vec<BlameLine> {
    let out = match run(root, &["blame", "--date=short", "--", path]) {
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
