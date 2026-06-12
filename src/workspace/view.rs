//! The chat tab shell: top bar, file tree + changes sidebar, editor-area tab
//! bar, transcript body, bottom menu bar, and the embedded terminal.

use std::time::Instant;

use eframe::egui;

use super::Workspace;
use super::diff::{file_name, marker_color};
use super::editor_area::url_host_port;
use super::render::{TreeActions, render_dir};
use super::state::{EditorItem, EditorSide};
use crate::browser::BrowserManager;

impl Workspace {
    pub fn render_chat(
        &mut self,
        ui: &mut egui::Ui,
        browser: &mut BrowserManager,
        composer_style: &mut crate::session::ComposerStyle,
    ) {
        let id = self.id.0;
        self.poll_git(ui.ctx());

        // Offer to open the workspace's dev server in the browser plugin, when
        // it's installed. Scan the embedded terminal's output for local URLs.
        let browser_available = browser.is_available();
        let dev_urls: Vec<String> = if browser_available {
            // Drop our own CDP endpoints and de-dupe by host:port so the chips
            // only show real dev servers (not the browser's debugging ports).
            let cdp = browser.cdp_hostports();
            let mut seen = std::collections::HashSet::new();
            crate::browser::detect_dev_urls(&self.dev_url_scan_text())
                .into_iter()
                .filter(|u| {
                    let hp = url_host_port(u);
                    !cdp.contains(&hp) && seen.insert(hp)
                })
                .collect()
        } else {
            Vec::new()
        };
        // Collected this frame: Some(url) opens a browser editor tab in-workspace.
        let mut open_browser: Option<Option<String>> = None;

        let mut open_git = false;
        let mut new_chat = false;
        let can_new_chat = self.ready && !self.running && !self.transcript.is_empty();
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
                if ui
                    .add_enabled(can_new_chat, egui::Button::new("\u{ff0b} New chat"))
                    .on_hover_text("Clear the conversation and start fresh")
                    .clicked()
                {
                    new_chat = true;
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
        if new_chat {
            // Reuses the /clear machinery: wipes the transcript (showing the
            // empty state) and resets the sidecar conversation.
            self.dispatch_command("/clear");
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

            let mut acts = TreeActions::default();
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

                    // File tree fills the remaining (top) space. Header carries
                    // explicit New-file/folder buttons (right-click menus on the
                    // folders below work too, but buttons are discoverable).
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("🗂 {}", self.name)).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("+ Folder")
                                .on_hover_text("New folder in the project root")
                                .clicked()
                            {
                                acts.new_in = Some((self.root.clone(), true));
                            }
                            if ui
                                .small_button("+ File")
                                .on_hover_text("New file in the project root")
                                .clicked()
                            {
                                acts.new_in = Some((self.root.clone(), false));
                            }
                        });
                    });
                    if let Some(label) = self.remote_label() {
                        // Remote workspace: tree + editor work over SSH.
                        ui.weak(format!("\u{1f517} {label}"));
                    }
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt(("tree-scroll", id))
                        .show(ui, |ui| {
                            render_dir(ui, &*self.fs, &self.root, &markers, &mut acts);
                        });
                });

            if do_refresh {
                self.git_refresh_at = Instant::now();
            }
            if let Some(path) = acts.open {
                self.open_editor_file(path);
            }
            if let Some(path) = acts.delete {
                self.pending_delete = Some(path);
                self.delete_error = None;
            }
            if let Some(path) = acts.rename {
                self.start_rename(path);
            }
            if let Some((parent, is_dir)) = acts.new_in {
                self.start_new(parent, is_dir);
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
            self.render_chat_body(ui, composer_style);
        } else {
            match self.editor_side {
                EditorSide::Bottom => {
                    // Stacked: editor on top, chat in a resizable bottom panel.
                    egui::Panel::bottom(egui::Id::new(("ws-chat", id)))
                        .resizable(true)
                        .default_size(280.0)
                        .show_inside(ui, |ui| {
                            self.render_chat_body(ui, composer_style);
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
                    self.render_chat_body(ui, composer_style);
                }
            }
        }

        // Self-clean: once no browser tab remains, remove the CDP breadcrumb so
        // it can't go stale (it's re-created when a browser is reopened).
        let has_browser = self
            .editor_open
            .iter()
            .any(|it| matches!(it, EditorItem::Browser(_)));
        if !has_browser && self.browser_cdp_written.is_some() {
            super::cleanup_puppy_browser(&self.root);
            self.browser_cdp_written = None;
        }

        // Interactive question modal floats above everything for this workspace.
        if self.pending_delete.is_some() {
            self.render_delete_modal(ui.ctx());
        }
        if self.pending_rename.is_some() {
            self.render_rename_modal(ui.ctx());
        }
        if self.pending_new.is_some() {
            self.render_new_modal(ui.ctx());
        }
        if self.git_creds.is_some() {
            self.render_git_creds_modal(ui.ctx());
        }
        if self.file_browser.is_some() {
            self.render_file_browser(ui.ctx());
        }
        if self.pending_ask.is_some() {
            self.render_ask_modal(ui.ctx());
        }
        if self.show_sessions {
            self.render_sessions_modal(ui.ctx());
        }
    }
}
