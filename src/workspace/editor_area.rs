//! The editor area above the chat: open-file/Changes/Git/browser tabs, the
//! embedded terminal, and the browser CDP plumbing. Split from `view.rs`
//! (mechanical move, no behavior change).

use eframe::egui;

use super::Workspace;
use super::state::{EditorItem, EditorSide, Entry};
use crate::browser::BrowserManager;

impl Workspace {
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
                                        .on_hover_text(
                                            "Preview this HTML file in the browser plugin",
                                        )
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
                EditorItem::Browser(bid) => {
                    // Publish the page's CDP endpoint so Code Puppy can attach.
                    if let Some(cdp) = browser.tab_cdp_url(bid) {
                        let url = browser.tab_url(bid).unwrap_or_default();
                        self.sync_browser_cdp_file(&cdp, &url);
                    }
                    browser.render_tab(ui, bid);
                }
            }
        }
    }

    /// Lazily spawn (or respawn) the workspace shell.
    pub(crate) fn spawn_terminal(&mut self, ctx: egui::Context) {
        match crate::terminal::Terminal::spawn(&self.root, crate::waker::egui_waker(&ctx)) {
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
    pub(super) fn open_browser_tab(&mut self, browser: &mut BrowserManager, url: Option<String>) {
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

    /// Write the in-app browser's CDP endpoint to `<root>/.puppy/browser.json`
    /// so Code Puppy (cwd = workspace root) can discover and attach to it when
    /// asked to inspect the page. Only rewritten when the endpoint changes.
    fn sync_browser_cdp_file(&mut self, cdp: &str, url: &str) {
        if self.browser_cdp_written.as_deref() == Some(cdp) {
            return;
        }
        let dir = self.root.join(".puppy");
        if std::fs::create_dir_all(&dir).is_ok() {
            // Keep this transient runtime dir out of git (self-contained ignore,
            // so we never touch the user's root .gitignore). Only if it's a repo.
            if self.root.join(".git").exists() {
                let gi = dir.join(".gitignore");
                if !gi.exists() {
                    let _ = std::fs::write(&gi, "*\n");
                }
            }
            // Ready-made CDP helper path (JSON-escape Windows backslashes).
            let helper = crate::browser::ensure_cdp_helper()
                .map(|p| p.display().to_string().replace('\\', "\\\\"))
                .unwrap_or_default();
            let body = format!(
                "{{\n  \"cdp\": \"{cdp}\",\n  \"url\": \"{url}\",\n  \"helper\": \"{helper}\",\n  \"hint\": \"Run the helper to inspect the page: python <helper> {cdp} console|eval|screenshot. Or drive CDP directly: GET {cdp}/json/list for webSocketDebuggerUrl.\"\n}}\n"
            );
            if std::fs::write(dir.join("browser.json"), body).is_ok() {
                self.browser_cdp_written = Some(cdp.to_string());
            }
        }
    }

    /// Text to scan for dev-server URLs: the embedded terminal's screen plus the
    /// recent transcript (the agent often prints "running at http://localhost…").
    pub(super) fn dev_url_scan_text(&self) -> String {
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
pub(super) fn url_host_port(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    after
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after)
        .to_string()
}
