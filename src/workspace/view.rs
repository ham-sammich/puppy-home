//! The chat tab shell: top bar, file tree + changes sidebar, editor-area tab
//! bar, transcript body, bottom menu bar, and the embedded terminal.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use super::Workspace;
use super::diff::{file_name, marker_color};
use super::render::{render_dir, render_entry};
use super::state::{EditorItem, EditorSide, Entry};
use crate::browser::BrowserManager;

impl Workspace {
    pub fn render_chat(&mut self, ui: &mut egui::Ui, browser: &mut BrowserManager) {
        let id = self.id.0;
        self.poll_git(ui.ctx());

        // Offer to open the workspace's dev server in the browser plugin, when
        // it's installed. Scan the embedded terminal's output for local URLs.
        let browser_available = browser.is_available();
        let dev_urls: Vec<String> = if browser_available {
            crate::browser::detect_dev_urls(&self.dev_url_scan_text())
        } else {
            Vec::new()
        };
        // Collected this frame: Some(url) opens a browser editor tab in-workspace.
        let mut open_browser: Option<Option<String>> = None;

        let mut open_git = false;
        egui::Panel::top(egui::Id::new(("ws-top", id))).show_inside(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut self.show_tree, "🗂 Tree")
                    .on_hover_text("Toggle the file tree");
                if self.git_repo
                    && ui
                        .button("🌿 Git")
                        .on_hover_text("Source control")
                        .clicked()
                {
                    open_git = true;
                }
                ui.separator();
                self.render_puppy_name(ui);
                ui.separator();
                if browser_available {
                    if ui
                        .button("Browser")
                        .on_hover_text("Open a browser tab in this workspace")
                        .clicked()
                    {
                        open_browser = Some(None);
                    }
                    for url in &dev_urls {
                        let label = format!("Open {}", url_host_port(url));
                        if ui
                            .button(label)
                            .on_hover_text(format!("Open {url} in the browser plugin"))
                            .clicked()
                        {
                            open_browser = Some(Some(url.clone()));
                        }
                    }
                    ui.separator();
                }
                ui.label(egui::RichText::new(&self.status_line).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_logs, "logs");
                    if self.running {
                        if self.paused {
                            ui.colored_label(egui::Color32::from_rgb(220, 190, 110), "⏸ paused");
                        } else {
                            ui.spinner();
                        }
                    }
                });
            });
        });

        if open_git {
            self.show_git();
        }
        // Open the browser as an editor tab *inside* this workspace.
        if let Some(url) = open_browser {
            self.open_browser_tab(browser, url);
        }

        // File tree sidebar (toggleable) — explorer (top) + Changes (bottom).
        if self.show_tree {
            let markers = self.tree_markers();
            // Snapshot the change list so the closures don't borrow self.
            let git_repo = self.git_repo;
            let git_list: Vec<(String, char)> = if git_repo {
                self.git_changes
                    .iter()
                    .map(|c| (c.path.clone(), c.marker))
                    .collect()
            } else {
                Vec::new()
            };
            let diff_list = if git_repo {
                Vec::new()
            } else {
                self.diff_changed_files()
            };
            let count = if git_repo {
                git_list.len()
            } else {
                diff_list.len()
            };

            let mut open_file: Option<PathBuf> = None;
            let mut click_diff: Option<usize> = None;
            let mut click_git: Option<(String, char)> = None;
            let mut do_refresh = false;

            egui::Panel::left(egui::Id::new(("ws-tree", id)))
                .resizable(true)
                .default_size(240.0)
                .show_inside(ui, |ui| {
                    // Source-control style Changes panel, pinned to the bottom.
                    egui::Panel::bottom(egui::Id::new(("ws-changes", id)))
                        .resizable(true)
                        .default_size(160.0)
                        .show_inside(ui, |ui| {
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Changes ({count})")).strong(),
                                );
                                if git_repo {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button("⟳")
                                                .on_hover_text("Refresh")
                                                .clicked()
                                            {
                                                do_refresh = true;
                                            }
                                        },
                                    );
                                }
                            });
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .id_salt(("changes-scroll", id))
                                .show(ui, |ui| {
                                    if count == 0 {
                                        ui.weak(if git_repo {
                                            "Working tree clean."
                                        } else {
                                            "No changes yet."
                                        });
                                    }
                                    if git_repo {
                                        for (path, marker) in &git_list {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    marker_color(*marker),
                                                    marker.to_string(),
                                                );
                                                if ui
                                                    .selectable_label(false, file_name(path))
                                                    .on_hover_text(path)
                                                    .clicked()
                                                {
                                                    click_git = Some((path.clone(), *marker));
                                                }
                                            });
                                        }
                                    } else {
                                        for (idx, path, marker) in &diff_list {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    marker_color(*marker),
                                                    marker.to_string(),
                                                );
                                                if ui
                                                    .selectable_label(false, file_name(path))
                                                    .on_hover_text(path)
                                                    .clicked()
                                                {
                                                    click_diff = Some(*idx);
                                                }
                                            });
                                        }
                                    }
                                });
                        });

                    // File tree fills the remaining (top) space.
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("🗂 {}", self.name)).strong());
                    ui.separator();
                    let mut clicked: Option<PathBuf> = None;
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt(("tree-scroll", id))
                        .show(ui, |ui| {
                            render_dir(ui, &self.root, &markers, &mut clicked);
                        });
                    if let Some(path) = clicked {
                        open_file = Some(path);
                    }
                });

            if do_refresh {
                self.git_refresh_at = Instant::now();
            }
            if let Some(path) = open_file {
                self.open_editor_file(path);
            }
            if let Some(i) = click_diff {
                self.load_diff_index(i);
            }
            if let Some((path, marker)) = click_git {
                self.load_git_diff(&path, marker);
            }
        }

        // Layout: with files/changes open, the editor fills the top and the
        // chat is pushed into a resizable bottom panel (IDE style). With nothing
        // open, the chat fills the whole area.
        if self.editor_open.is_empty() {
            self.render_chat_body(ui);
        } else {
            match self.editor_side {
                EditorSide::Bottom => {
                    // Stacked: editor on top, chat in a resizable bottom panel.
                    egui::Panel::bottom(egui::Id::new(("ws-chat", id)))
                        .resizable(true)
                        .default_size(280.0)
                        .show_inside(ui, |ui| {
                            self.render_chat_body(ui);
                        });
                    self.render_editor_area(ui, browser);
                }
                EditorSide::Right => {
                    // Side by side: editor on the right, chat fills the left.
                    egui::Panel::right(egui::Id::new(("ws-editor-side", id)))
                        .resizable(true)
                        .default_size(ui.available_width() * 0.5)
                        .show_inside(ui, |ui| {
                            self.render_editor_area(ui, browser);
                        });
                    self.render_chat_body(ui);
                }
            }
        }

        // Interactive question modal floats above everything for this workspace.
        if self.pending_ask.is_some() {
            self.render_ask_modal(ui.ctx());
        }
        if self.show_sessions {
            self.render_sessions_modal(ui.ctx());
        }
    }

    /// The chat region: transcript (scrolling) with the composer pinned to the
    /// bottom and the optional logs panel above it.
    pub(crate) fn render_chat_body(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;

        // Bottom-pinned controls: the chat composer (chat mode only) plus the
        // always-visible bottom menu bar (terminal toggle + agent + model).
        egui::Panel::bottom(egui::Id::new(("ws-composer", id))).show_inside(ui, |ui| {
            ui.add_space(4.0);
            if !self.show_terminal {
                if self.pending.is_some() {
                    self.render_pending(ui);
                } else {
                    self.render_composer(ui);
                }
                ui.add_space(2.0);
            }
            self.render_bottom_bar(ui);
            ui.add_space(4.0);
        });

        if self.show_logs {
            egui::Panel::bottom(egui::Id::new(("ws-logs", id)))
                .resizable(true)
                .default_size(120.0)
                .show_inside(ui, |ui| {
                    ui.label(egui::RichText::new("sidecar logs").weak());
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .id_salt(("ws-logs-scroll", id))
                        .show(ui, |ui| {
                            for line in &self.logs {
                                ui.label(egui::RichText::new(line).monospace().small());
                            }
                        });
                });
        }

        // Main area: the embedded terminal, or the chat transcript.
        if self.show_terminal {
            self.render_terminal(ui);
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .id_salt(("ws-transcript", id))
                .show(ui, |ui| {
                    if self.transcript_collapsed > 0 {
                        ui.weak(format!(
                            "{} earlier message(s) trimmed to keep the UI responsive.",
                            self.transcript_collapsed
                        ));
                    }
                    if self.transcript.is_empty() {
                        ui.weak(format!(
                            "Ask {} to build, edit, or explain code.",
                            self.puppy_name
                        ));
                    }
                    // Namespace each entry's widget ids (commonmark tables use a
                    // Grid) so repeated/duplicate content doesn't clash.
                    for (i, entry) in self.transcript.iter().enumerate() {
                        ui.push_id(("entry", i), |ui| {
                            render_entry(ui, entry, &mut self.md_cache, &self.puppy_name);
                        });
                    }
                });
        }
    }

    /// The editor area: a tab bar of open files / Changes, then the active one.
    pub(crate) fn render_editor_area(&mut self, ui: &mut egui::Ui, browser: &mut BrowserManager) {
        let id = self.id.0;
        let mut switch_to: Option<usize> = None;
        let mut close: Option<usize> = None;

        // Drop browser tabs whose process has exited (the user closed the page),
        // so a closed browser doesn't leave a dead tab behind.
        let dead: Vec<usize> = self
            .editor_open
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                EditorItem::Browser(b) if browser.is_tab_closed(*b) => Some(i),
                _ => None,
            })
            .collect();
        for i in dead.into_iter().rev() {
            if let Some(EditorItem::Browser(b)) = self.editor_open.get(i) {
                browser.close_tab(*b);
            }
            self.close_editor(i);
        }

        // Precompute tab labels so the tab-strip closure doesn't borrow `browser`.
        let labels: Vec<String> = self
            .editor_open
            .iter()
            .map(|item| match item {
                EditorItem::Changes => "📝 Changes".to_string(),
                EditorItem::Git => "🌿 Git".to_string(),
                EditorItem::Commit { short, .. } => format!("⎇ {short}"),
                EditorItem::Browser(bid) => browser.tab_title(*bid),
                EditorItem::File(p) => {
                    let dirty = self.open_files.get(p).map(|b| b.dirty).unwrap_or(false);
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string_lossy().into_owned());
                    format!("{}{name}", if dirty { "● " } else { "" })
                }
            })
            .collect();

        let mut toggle_side = false;
        egui::Panel::top(egui::Id::new(("ws-editortabs", id))).show_inside(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                // Right-aligned layout toggle: stack vs. side-by-side with chat.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match self.editor_side {
                        EditorSide::Bottom => "⬌ Side by side",
                        EditorSide::Right => "⬍ Stacked",
                    };
                    if ui
                        .small_button(label)
                        .on_hover_text("Move the editor/browser beside the chat, or back on top")
                        .clicked()
                    {
                        toggle_side = true;
                    }
                    // Tabs fill the remaining space, left-to-right.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        for (i, label) in labels.iter().enumerate() {
                            let selected = i == self.editor_active;
                            let label = label.clone();
                            ui.scope(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                if ui.selectable_label(selected, label).clicked() {
                                    switch_to = Some(i);
                                }
                                if ui.small_button("✕").clicked() {
                                    close = Some(i);
                                }
                                ui.separator();
                            });
                        }
                    });
                });
            });
        });

        if toggle_side {
            self.editor_side = match self.editor_side {
                EditorSide::Bottom => EditorSide::Right,
                EditorSide::Right => EditorSide::Bottom,
            };
        }
        if let Some(i) = switch_to {
            self.editor_active = i;
        }
        if let Some(i) = close {
            // Closing a browser tab also shuts down its plugin process.
            if let Some(EditorItem::Browser(bid)) = self.editor_open.get(i) {
                browser.close_tab(*bid);
            }
            self.close_editor(i);
        }

        if let Some(item) = self.editor_open.get(self.editor_active).cloned() {
            match item {
                EditorItem::Changes => self.render_diffs(ui),
                EditorItem::File(p) => {
                    // For HTML, offer a one-click preview in the browser plugin.
                    if browser.is_available() && is_html(&p) {
                        let mut preview = false;
                        egui::Panel::top(egui::Id::new(("ws-html-bar", id))).show_inside(
                            ui,
                            |ui| {
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .button("Open in browser")
                                        .on_hover_text("Preview this HTML file in the browser plugin")
                                        .clicked()
                                    {
                                        preview = true;
                                    }
                                });
                                ui.add_space(2.0);
                            },
                        );
                        if preview {
                            self.open_browser_tab(browser, Some(crate::browser::file_url(&p)));
                        }
                    }
                    self.render_file(ui, &p);
                }
                EditorItem::Git => self.render_git(ui),
                EditorItem::Commit { hash, .. } => self.render_commit(ui, &hash),
                EditorItem::Browser(bid) => browser.render_tab(ui, bid),
            }
        }
    }

    /// The bottom menu bar (always shown): terminal toggle + agent + model.
    pub(crate) fn render_bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let term_live = self.terminal.as_ref().map(|t| t.alive).unwrap_or(false);
            let label = if self.show_terminal {
                "🖥 Terminal ▾"
            } else {
                "🖥 Terminal"
            };
            let resp = ui
                .selectable_label(self.show_terminal, label)
                .on_hover_text("Toggle an embedded shell in the chat area");
            if resp.clicked() {
                self.show_terminal = !self.show_terminal;
                if self.show_terminal && self.terminal.is_none() {
                    self.spawn_terminal(ui.ctx().clone());
                }
            }
            if self.show_terminal && !term_live && self.terminal.is_some() {
                ui.colored_label(egui::Color32::from_gray(150), "(exited)");
            }
            if ui
                .selectable_label(self.show_sessions, "🗂 Sessions")
                .on_hover_text("Browse & resume saved Code Puppy conversations")
                .clicked()
            {
                self.show_sessions = !self.show_sessions;
                if self.show_sessions
                    && let Some(backend) = &self.backend
                {
                    backend.list_sessions();
                }
            }
            ui.separator();
            self.render_agent_picker(ui);
            self.render_model_picker(ui);
        });
    }

    /// Lazily spawn (or respawn) the workspace shell.
    pub(crate) fn spawn_terminal(&mut self, ctx: egui::Context) {
        match crate::terminal::Terminal::spawn(&self.root, ctx) {
            Ok(t) => self.terminal = Some(t),
            Err(e) => self.status_line = format!("Couldn't start terminal: {e}"),
        }
    }

    /// The embedded PTY terminal: a thin status bar + the live cell grid.
    pub(crate) fn render_terminal(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;
        let mut do_restart = false;
        egui::Panel::top(egui::Id::new(("ws-term-bar", id))).show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let alive = self.terminal.as_ref().map(|t| t.alive).unwrap_or(false);
                ui.label(egui::RichText::new("🖥 terminal").weak().small());
                if alive {
                    ui.label(
                        egui::RichText::new("click to focus · Ctrl+C interrupts")
                            .weak()
                            .small(),
                    );
                } else if self.terminal.is_some() {
                    ui.colored_label(egui::Color32::from_gray(160), "shell exited");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("⟳")
                        .on_hover_text("Restart the shell")
                        .clicked()
                    {
                        do_restart = true;
                    }
                });
            });
            ui.add_space(2.0);
        });

        if do_restart {
            self.spawn_terminal(ui.ctx().clone());
        }

        if let Some(term) = self.terminal.as_mut() {
            term.ui(ui);
        } else {
            ui.centered_and_justified(|ui| {
                ui.weak("starting shell…");
            });
        }
    }
}

impl Workspace {
    /// Open (or focus) a browser editor tab in this workspace. If a tab for the
    /// same URL is already open, focus it instead of opening a duplicate.
    fn open_browser_tab(&mut self, browser: &mut BrowserManager, url: Option<String>) {
        let existing = url.as_ref().and_then(|u| {
            self.editor_open.iter().position(|it| {
                matches!(it, EditorItem::Browser(b)
                    if browser.tab_url(*b).as_deref() == Some(u.as_str()))
            })
        });
        match existing {
            Some(i) => self.editor_active = i,
            None => {
                let bid = browser.open_tab(Some(self.id), url);
                self.focus_or_open(EditorItem::Browser(bid));
            }
        }
    }

    /// Text to scan for dev-server URLs: the embedded terminal's screen plus the
    /// recent transcript (the agent often prints "running at http://localhost…").
    fn dev_url_scan_text(&self) -> String {
        let mut s = String::new();
        if let Some(t) = &self.terminal {
            s.push_str(&t.screen_text());
            s.push('\n');
        }
        for e in self.transcript.iter().rev().take(40) {
            match e {
                Entry::Agent(t) | Entry::Note(t) | Entry::User(t) | Entry::Error(t) => {
                    s.push_str(t);
                    s.push('\n');
                }
                Entry::Thinking { text, .. } => {
                    s.push_str(text);
                    s.push('\n');
                }
                Entry::Message(_) => {}
            }
        }
        s
    }
}

/// Whether a path looks like an HTML file we can preview in the browser.
fn is_html(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    )
}

/// Compact `host:port` of a URL, for the dev-server chips.
fn url_host_port(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    after
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after)
        .to_string()
}
