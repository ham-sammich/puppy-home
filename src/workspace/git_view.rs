//! The Source Control / Git page, commit view, working-tree polling, and the
//! diff/changes plumbing that feeds the file tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;

use super::Workspace;
use super::diff::{
    DiffLine, DiffRecord, file_name, marker_color, op_marker, parse_unified, render_diff_lines,
};
use super::state::{EditorItem, GitAuthOp, GitView};

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

    /// Poll/refresh git working-tree status (off the UI thread).
    pub(crate) fn poll_git(&mut self, ctx: &egui::Context) {
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
        // Each poll spawns a `git status` process; on Windows that means exe
        // spawn + Defender scanning the repo, so: a wider cadence, and no
        // polling at all while the window is unfocused (nothing to show).
        let focused = ctx.input(|i| i.focused);
        if !self.git_pending && focused && Instant::now() >= self.git_refresh_at {
            let git = self.git.clone();
            let ctx2 = ctx.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.git_rx = Some(rx);
            self.git_pending = true;
            self.git_refresh_at = Instant::now() + std::time::Duration::from_millis(4000);
            std::thread::spawn(move || {
                let _ = tx.send(git.status());
                ctx2.request_repaint();
            });
        }
        // Keep the poll alive while this workspace is on screen (the wake
        // matches the poll cadence -- no busier).
        ctx.request_repaint_after(std::time::Duration::from_secs(4));
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

    /// The Git (Source Control) page: branch, staging, commit, history.
    pub(crate) fn render_git(&mut self, ui: &mut egui::Ui) {
        if self.git_view.is_none() {
            self.refresh_git_view();
        }
        let id = self.id.0;

        // Snapshot the view so the UI closures don't borrow `self`.
        let (branch, upstream, ahead, behind) = {
            let v = self.git_view.as_ref().unwrap();
            (v.branch.clone(), v.upstream, v.ahead, v.behind)
        };
        let staged: Vec<(String, char)> = self
            .git_view
            .as_ref()
            .unwrap()
            .staged
            .iter()
            .map(|e| (e.path.clone(), e.marker()))
            .collect();
        let unstaged: Vec<(String, char)> = self
            .git_view
            .as_ref()
            .unwrap()
            .unstaged
            .iter()
            .map(|e| (e.path.clone(), e.marker()))
            .collect();
        let log = self.git_view.as_ref().unwrap().log.clone();
        let action_msg = self.git_action_msg.clone();
        let can_commit = !staged.is_empty() && !self.commit_msg.trim().is_empty();

        let mut do_refresh = false;
        let mut do_fetch = false;
        let mut do_pull = false;
        let mut do_push = false;
        let mut do_commit = false;
        let mut do_stage_all = false;
        let mut do_unstage_all = false;
        let mut stage_path: Option<String> = None;
        let mut unstage_path: Option<String> = None;
        let mut diff_click: Option<(String, char)> = None;
        let mut commit_click: Option<crate::git::Commit> = None;

        // Branch header.
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("⎇ {branch}")).strong());
            if upstream {
                if ahead > 0 {
                    ui.label(egui::RichText::new(format!("↑{ahead}")).small());
                }
                if behind > 0 {
                    ui.label(egui::RichText::new(format!("↓{behind}")).small());
                }
                if ahead == 0 && behind == 0 {
                    ui.label(egui::RichText::new("up to date").weak().small());
                }
            } else {
                ui.label(egui::RichText::new("no upstream").weak().small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                    do_refresh = true;
                }
                if ui.small_button("Push").on_hover_text("git push").clicked() {
                    do_push = true;
                }
                if ui
                    .small_button("Pull")
                    .on_hover_text("git pull --ff-only")
                    .clicked()
                {
                    do_pull = true;
                }
                if ui
                    .small_button("Fetch")
                    .on_hover_text("git fetch --all --prune")
                    .clicked()
                {
                    do_fetch = true;
                }
                // List <-> Graph toggle (GitKraken-style commit tree).
                let (next, label, tip) = if self.git_show_graph {
                    (false, "List", "Show the flat history list")
                } else {
                    (true, "Graph", "Show the commit graph")
                };
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    self.git_show_graph = next;
                }
            });
        });
        if let Some((ok, msg)) = &action_msg {
            let color = if *ok {
                egui::Color32::from_rgb(120, 200, 140)
            } else {
                egui::Color32::from_rgb(230, 120, 120)
            };
            ui.colored_label(color, msg);
        }
        ui.separator();

        // Commit message area — drag the strip under it to give the box more
        // room. Its height is OUR state, changed only by an actual drag: an
        // egui resizable panel's persisted height can creep a pixel per
        // repaint under fractional DPI scaling (Windows at 125%/150%), which
        // made this box slowly grow on its own; integer-scaled macOS never
        // showed it.
        let commit_h = self.commit_box_h.clamp(52.0, 320.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), commit_h), |ui| {
            ui.set_min_size(egui::vec2(ui.available_width(), commit_h));
            {
                ui.add_space(4.0);
                // Button row pinned to the bottom; the message box fills above it.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(can_commit, egui::Button::new("✓ Commit"))
                            .on_hover_text(if staged.is_empty() {
                                "Stage something first"
                            } else {
                                "Commit staged changes"
                            })
                            .clicked()
                        {
                            do_commit = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("{} staged", staged.len()))
                                .weak()
                                .small(),
                        );
                    });
                    ui.add_space(4.0);
                    // Fill the space above the buttons MINUS a little slack.
                    // If the box exactly fills the leftover height, any margin/
                    // rounding overflow raises the resizable panel's min-content
                    // height, which feeds back into a taller panel (and box)
                    // on the next repaint -- the box visibly grew on its own.
                    // The slack plus a 1-row intrinsic minimum (default is 4
                    // rows, taller than the panel's default) keep the content
                    // strictly inside the panel, so the loop can't start.
                    let box_h = (ui.available_height() - 8.0).max(36.0);
                    ui.add_sized(
                        egui::vec2(ui.available_width(), box_h),
                        egui::TextEdit::multiline(&mut self.commit_msg)
                            .id_salt(("commit-msg", id))
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .hint_text("Commit message…"),
                    );
                });
            }
        });
        // The drag strip that resizes the commit box (replaces the old
        // resizable-panel edge; only a real drag ever changes the height).
        let (strip, strip_resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
        if strip_resp.hovered() || strip_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if strip_resp.dragged() {
            self.commit_box_h = (self.commit_box_h + strip_resp.drag_delta().y).clamp(52.0, 320.0);
        }
        ui.painter().hline(
            strip.x_range(),
            strip.center().y,
            ui.visuals().widgets.noninteractive.bg_stroke,
        );

        // Staged + changed files — drag its bottom edge to resize the list.
        egui::Panel::top(egui::Id::new(("git-stage", id)))
            .resizable(true)
            .min_size(64.0)
            .default_size(190.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt(("git-stage-scroll", id))
                    .show(ui, |ui| {
                        // Staged.
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Staged ({})", staged.len())).strong(),
                            );
                            if !staged.is_empty() && ui.small_button("Unstage all").clicked() {
                                do_unstage_all = true;
                            }
                        });
                        if staged.is_empty() {
                            ui.weak("Nothing staged.");
                        }
                        for (path, marker) in &staged {
                            ui.horizontal(|ui| {
                                if ui.small_button("−").on_hover_text("Unstage").clicked() {
                                    unstage_path = Some(path.clone());
                                }
                                ui.colored_label(marker_color(*marker), marker.to_string());
                                if ui
                                    .selectable_label(false, file_name(path))
                                    .on_hover_text(path)
                                    .clicked()
                                {
                                    diff_click = Some((path.clone(), *marker));
                                }
                            });
                        }

                        ui.add_space(6.0);

                        // Unstaged / untracked.
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Changes ({})", unstaged.len()))
                                    .strong(),
                            );
                            if !unstaged.is_empty() && ui.small_button("Stage all").clicked() {
                                do_stage_all = true;
                            }
                        });
                        if unstaged.is_empty() {
                            ui.weak("No unstaged changes.");
                        }
                        for (path, marker) in &unstaged {
                            ui.horizontal(|ui| {
                                if ui.small_button("+").on_hover_text("Stage").clicked() {
                                    stage_path = Some(path.clone());
                                }
                                ui.colored_label(marker_color(*marker), marker.to_string());
                                if ui
                                    .selectable_label(false, file_name(path))
                                    .on_hover_text(path)
                                    .clicked()
                                {
                                    diff_click = Some((path.clone(), *marker));
                                }
                            });
                        }
                    });
            });

        // History fills the remaining space below the resizable sections.
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("History").strong());
                ui.label(
                    egui::RichText::new(if self.git_show_graph {
                        "(graph — all branches)"
                    } else {
                        "(current branch)"
                    })
                    .weak()
                    .small(),
                );
            });

            if self.git_show_graph {
                // GitKraken-style commit tree (owns its own scroll area).
                self.render_graph(ui);
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .id_salt(("git-log-scroll", id))
                    .show(ui, |ui| {
                        for c in &log {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&c.short)
                                        .monospace()
                                        .small()
                                        .color(egui::Color32::from_rgb(180, 150, 220)),
                                );
                                if ui
                                    .selectable_label(false, &c.subject)
                                    .on_hover_text(format!("{} · {}", c.author, c.when))
                                    .clicked()
                                {
                                    commit_click = Some(c.clone());
                                }
                            });
                        }
                    });
            }
        });

        // Apply deferred actions.
        if do_refresh {
            self.git_action_msg = None;
            self.refresh_git_view();
        }
        if do_fetch {
            match self.git.fetch() {
                Ok(s) => self.git_action(
                    Ok(()),
                    if s.is_empty() {
                        "Fetched (up to date)"
                    } else {
                        "Fetched from remotes"
                    },
                ),
                Err(e) => self.git_net_error(e, GitAuthOp::Fetch),
            }
        }
        if do_pull {
            match self.git.pull() {
                Ok(s) => {
                    let line = s.lines().last().unwrap_or("Pulled").to_string();
                    self.git_action(Ok(()), &format!("Pulled · {line}"));
                }
                Err(e) => self.git_net_error(e, GitAuthOp::Pull),
            }
        }
        if do_push {
            match self.git.push() {
                Ok(_) => self.git_action(Ok(()), "Pushed to upstream"),
                Err(e) => self.git_net_error(e, GitAuthOp::Push),
            }
        }
        if let Some(p) = stage_path {
            let r = self.git.stage(&p);
            self.git_action(r, &format!("Staged {}", file_name(&p)));
        }
        if let Some(p) = unstage_path {
            let r = self.git.unstage(&p);
            self.git_action(r, &format!("Unstaged {}", file_name(&p)));
        }
        if do_stage_all {
            let r = self.git.stage_all();
            self.git_action(r, "Staged all changes");
        }
        if do_unstage_all {
            let r = self.git.unstage_all();
            self.git_action(r, "Unstaged all");
        }
        if do_commit {
            let msg = self.commit_msg.clone();
            match self.git.commit(&msg) {
                Ok(summary) => {
                    self.commit_msg.clear();
                    let line = summary.lines().next().unwrap_or("committed").to_string();
                    self.git_action(Ok(()), &format!("Committed · {line}"));
                }
                Err(e) => self.git_action(Err(e), ""),
            }
        }
        if let Some((path, marker)) = diff_click {
            self.load_git_diff(&path, marker);
        }
        if let Some(c) = commit_click {
            self.open_commit(&c);
        }
    }

    /// A single commit's patch (opened from the Git history list).
    pub(crate) fn render_commit(&mut self, ui: &mut egui::Ui, hash: &str) {
        let id = self.id.0;
        let matches = self
            .commit_view
            .as_ref()
            .map(|(h, _)| h == hash)
            .unwrap_or(false);
        if !matches {
            // A different commit tab is active; re-fetch this one.
            let text = self.git.show(hash);
            let (lines, adds, dels) = parse_unified(&text);
            self.commit_view = Some((
                hash.to_string(),
                DiffRecord {
                    path: hash.chars().take(8).collect(),
                    operation: "modify".to_string(),
                    adds,
                    dels,
                    lines,
                },
            ));
        }
        let Some((_, d)) = &self.commit_view else {
            return;
        };
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&d.path).strong());
            ui.label(
                egui::RichText::new(format!("+{}  −{}", d.adds, d.dels))
                    .weak()
                    .small(),
            );
        });
        ui.separator();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("commit-scroll", id, hash))
            .show(ui, |ui| {
                render_diff_lines(ui, &d.lines);
            });
    }
}
