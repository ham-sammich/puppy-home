//! The Source Control / Git page, commit view, working-tree polling, and the
//! diff/changes plumbing that feeds the file tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::Workspace;
use super::diff::{DiffLine, DiffRecord, file_name, op_marker, parse_unified};
use super::state::{EditorItem, GitView};

impl Workspace {
    /// Map each changed file (absolute path) to its change marker, for inline
    /// tree badges. Uses git working-tree status when the folder is a repo,
    /// otherwise falls back to Code-Puppy-reported diffs.
    pub(crate) fn tree_markers(&self) -> HashMap<PathBuf, char> {
        let mut map = HashMap::new();
        if self.git_repo {
            for c in &self.git_changes {
                map.insert(self.root.join(&c.path), c.marker);
            }
        } else {
            for d in &self.diffs {
                map.insert(self.abs_path(&d.path), op_marker(&d.operation));
            }
        }
        map
    }

    /// Code-Puppy-reported changes (non-git fallback), newest first:
    /// (diff index, path, marker).
    pub(crate) fn diff_changed_files(&self) -> Vec<(usize, String, char)> {
        let mut latest: HashMap<&str, (usize, char)> = HashMap::new();
        for (i, d) in self.diffs.iter().enumerate() {
            latest.insert(d.path.as_str(), (i, op_marker(&d.operation)));
        }
        let mut out: Vec<(usize, String, char)> = latest
            .into_iter()
            .map(|(p, (i, m))| (i, p.to_string(), m))
            .collect();
        out.sort_by_key(|b| std::cmp::Reverse(b.0));
        out
    }

    /// Frontend-agnostic git status poll (the egui `poll_git` minus the
    /// `egui::Context`): receive a finished background `git status`, and
    /// kick a new one at most every 4s while the window is focused. The
    /// caller's drain loop provides the cadence; the waker provides the
    /// repaint when the thread finishes.
    pub(crate) fn poll_git_status(
        &mut self,
        focused: bool,
        waker: &std::sync::Arc<dyn crate::waker::UiWaker>,
    ) {
        if !self.git_repo {
            return;
        }
        if let Some(rx) = &self.git_rx
            && let Ok(changes) = rx.try_recv()
        {
            self.git_changes = changes;
            self.git_pending = false;
            self.git_rx = None;
        }
        if !self.git_pending && focused && Instant::now() >= self.git_refresh_at {
            let git = self.git.clone();
            let waker = waker.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.git_rx = Some(rx);
            self.git_pending = true;
            self.git_refresh_at = Instant::now() + std::time::Duration::from_millis(4000);
            std::thread::spawn(move || {
                let _ = tx.send(git.status());
                waker.wake();
            });
        }
    }

    // -- frontend-agnostic git actions (the egui render_git flag tail,
    //    extracted so the GPUI panel drives the same logic; sync batch) --

    pub(crate) fn git_view_data(&self) -> Option<&GitView> {
        self.git_view.as_ref()
    }

    pub(crate) fn git_action_message(&self) -> Option<&(bool, String)> {
        self.git_action_msg.as_ref()
    }

    pub(crate) fn graph_commits(&self) -> &[crate::git::Commit] {
        &self.git_graph_commits
    }

    pub(crate) fn commit_view_data(&self) -> Option<&(String, DiffRecord)> {
        self.commit_view.as_ref()
    }

    pub(crate) fn blame_lines(&self, path: &Path) -> Option<&Vec<crate::git::BlameLine>> {
        self.blame_cache.get(path)
    }

    pub(crate) fn blame_enabled(&self, path: &Path) -> bool {
        self.blame_files.contains(path)
    }

    pub(crate) fn git_fetch(&mut self) {
        match self.git.fetch() {
            Ok(s) => self.git_action(
                Ok(()),
                if s.is_empty() {
                    "Fetched (up to date)"
                } else {
                    "Fetched from remotes"
                },
            ),
            Err(e) => self.git_net_error(e, crate::workspace::state::GitAuthOp::Fetch),
        }
    }

    pub(crate) fn git_pull(&mut self) {
        match self.git.pull() {
            Ok(s) => {
                let line = s.lines().last().unwrap_or("Pulled").to_string();
                self.git_action(Ok(()), &format!("Pulled \u{b7} {line}"));
            }
            Err(e) => self.git_net_error(e, crate::workspace::state::GitAuthOp::Pull),
        }
    }

    pub(crate) fn git_push(&mut self) {
        match self.git.push() {
            Ok(_) => self.git_action(Ok(()), "Pushed to upstream"),
            Err(e) => self.git_net_error(e, crate::workspace::state::GitAuthOp::Push),
        }
    }

    pub(crate) fn git_stage_path(&mut self, path: &str) {
        let r = self.git.stage(path);
        self.git_action(r, &format!("Staged {}", file_name(path)));
    }

    pub(crate) fn git_unstage_path(&mut self, path: &str) {
        let r = self.git.unstage(path);
        self.git_action(r, &format!("Unstaged {}", file_name(path)));
    }

    pub(crate) fn git_stage_all(&mut self) {
        let r = self.git.stage_all();
        self.git_action(r, "Staged all changes");
    }

    pub(crate) fn git_unstage_all(&mut self) {
        let r = self.git.unstage_all();
        self.git_action(r, "Unstaged all");
    }

    /// Commit with an explicit message (the GPUI commit box owns its text).
    /// Returns true on success so the caller can clear its input.
    pub(crate) fn git_commit_msg(&mut self, msg: &str) -> bool {
        match self.git.commit(msg) {
            Ok(summary) => {
                let line = summary.lines().next().unwrap_or("committed").to_string();
                self.git_action(Ok(()), &format!("Committed \u{b7} {line}"));
                true
            }
            Err(e) => {
                self.git_action(Err(e), "");
                false
            }
        }
    }

    pub(crate) fn git_checkout(&mut self, name: &str) {
        let r = self.git.checkout(name);
        self.git_action(r, &format!("Checked out {name}"));
    }

    pub(crate) fn git_merge(&mut self, target: &str) {
        match self.git.merge(target) {
            Ok(s) => {
                let line = s.lines().next().unwrap_or("merged").to_string();
                self.git_action(Ok(()), &format!("Merged \u{b7} {line}"));
            }
            Err(e) => self.git_action(Err(e), ""),
        }
    }

    pub(crate) fn git_create_branch(&mut self, name: &str, at: &str) {
        let r = self.git.create_branch(name, at);
        self.git_action(r, &format!("Created branch {name}"));
    }

    pub(crate) fn git_cherry_pick(&mut self, hash: &str) {
        let r = self.git.cherry_pick(hash);
        self.git_action(r, "Cherry-picked");
    }

    pub(crate) fn git_revert(&mut self, hash: &str) {
        let r = self.git.revert(hash);
        self.git_action(r, "Reverted");
    }

    pub(crate) fn git_reset(&mut self, hash: &str, mode: &str) {
        let r = self.git.reset(hash, mode);
        self.git_action(r, &format!("Reset ({mode}) to {hash}"));
    }

    /// Show the diff for a git-tracked change.
    pub(crate) fn load_git_diff(&mut self, path: &str, marker: char) {
        let (lines, adds, dels) = if marker == '?' {
            let content = self.git.untracked_content(path).unwrap_or_default();
            let lines: Vec<DiffLine> = content
                .lines()
                .map(|l| DiffLine {
                    kind: "add".into(),
                    content: l.to_string(),
                })
                .collect();
            let n = lines.len();
            (lines, n, 0)
        } else {
            parse_unified(&self.git.diff(path))
        };
        let operation = match marker {
            '?' | 'A' => "create",
            'D' => "delete",
            _ => "modify",
        }
        .to_string();
        self.current_diff = Some(DiffRecord {
            path: path.to_string(),
            operation,
            adds,
            dels,
            lines,
        });
        self.show_changes();
    }

    /// Show the diff for a Code-Puppy-reported change (non-git fallback).
    pub(crate) fn load_diff_index(&mut self, idx: usize) {
        if let Some(d) = self.diffs.get(idx) {
            self.current_diff = Some(d.clone());
            self.show_changes();
        }
    }

    /// Rebuild the cached Git-page snapshot (branch, staging, recent history).
    pub(crate) fn refresh_git_view(&mut self) {
        let info = self.git.head_info();
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        for e in self.git.status_full() {
            if e.is_staged() {
                staged.push(e.clone());
            }
            if e.is_unstaged() {
                unstaged.push(e);
            }
        }
        self.git_view = Some(GitView {
            branch: info.branch,
            upstream: info.upstream,
            ahead: info.ahead,
            behind: info.behind,
            staged,
            unstaged,
            log: self.git.log(50),
        });
        self.git_graph_commits = self.git.graph_log(200);
    }

    /// Open (or focus) the Source Control / Git page.
    pub fn show_git(&mut self) {
        self.refresh_git_view();
        self.focus_or_open(EditorItem::Git);
    }

    /// Open (or focus) a single commit's patch.
    pub(crate) fn open_commit(&mut self, c: &crate::git::Commit) {
        let text = self.git.show(&c.hash);
        let (lines, adds, dels) = parse_unified(&text);
        self.commit_view = Some((
            c.hash.clone(),
            DiffRecord {
                path: format!("{} {}", c.short, c.subject),
                operation: "modify".to_string(),
                adds,
                dels,
                lines,
            },
        ));
        self.focus_or_open(EditorItem::Commit {
            hash: c.hash.clone(),
            short: c.short.clone(),
            subject: c.subject.clone(),
        });
    }

    /// Toggle the inline blame gutter for a file (re-blames on each enable).
    pub(crate) fn toggle_blame(&mut self, path: &Path) {
        if self.blame_files.remove(path) {
            return; // was on → turned off
        }
        let target = path.to_string_lossy().into_owned();
        let blame = self.git.blame(&target);
        self.blame_cache.insert(path.to_path_buf(), blame);
        self.blame_files.insert(path.to_path_buf());
    }

    /// Run a staging action, then refresh the view + record feedback.
    pub(crate) fn git_action(&mut self, result: Result<(), String>, ok_msg: &str) {
        match result {
            Ok(()) => self.git_action_msg = Some((true, ok_msg.to_string())),
            Err(e) => self.git_action_msg = Some((false, e)),
        }
        self.refresh_git_view();
        self.git_refresh_at = Instant::now(); // refresh tree markers / Changes panel too
    }
}
