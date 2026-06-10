//! File buffers, editor file tabs, save/dirty/reload, and inline blame.

use std::path::{Path, PathBuf};

use eframe::egui;

use super::Workspace;
use super::render::truncate;
use super::state::{EditorItem, FileBuffer};

impl Workspace {
    /// Load a file into an editable buffer (no-op if already open).
    pub fn open_file(&mut self, path: PathBuf) {
        if self.open_files.contains_key(&path) {
            return;
        }
        let buffer = match std::fs::read_to_string(&path) {
            Ok(content) => FileBuffer {
                content,
                dirty: false,
                load_error: None,
                save_error: None,
            },
            Err(e) => FileBuffer {
                content: String::new(),
                dirty: false,
                load_error: Some(e.to_string()),
                save_error: None,
            },
        };
        self.open_files.insert(path, buffer);
    }

    /// Whether an open file has unsaved edits (for the tab title marker).
    #[allow(dead_code)] // accessor kept for tab-marker callers; inlined today
    pub fn is_file_dirty(&self, path: &Path) -> bool {
        self.open_files.get(path).map(|b| b.dirty).unwrap_or(false)
    }

    /// Open (or focus) a file in the editor area.
    pub fn open_editor_file(&mut self, path: PathBuf) {
        self.open_file(path.clone());
        let item = EditorItem::File(path);
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    /// Open (or focus) the Changes (diff) tab in the editor area.
    pub fn show_changes(&mut self) {
        match self
            .editor_open
            .iter()
            .position(|t| *t == EditorItem::Changes)
        {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(EditorItem::Changes);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    pub(crate) fn close_editor(&mut self, index: usize) {
        if index >= self.editor_open.len() {
            return;
        }
        self.editor_open.remove(index);
        if self.editor_active >= self.editor_open.len() {
            self.editor_active = self.editor_open.len().saturating_sub(1);
        }
    }

    /// Confirm + perform a delete requested from the file-tree context menu.
    pub(crate) fn render_delete_modal(&mut self, ctx: &egui::Context) {
        let Some(path) = self.pending_delete.clone() else {
            return;
        };
        let is_dir = path.is_dir();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        // 0 = nothing, 1 = delete, 2 = cancel.
        let mut action = 0u8;
        egui::Window::new("Delete")
            .id(egui::Id::new(("delete-modal", self.id.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                if is_dir {
                    ui.label(format!(
                        "Delete the folder \u{201c}{name}\u{201d} and everything inside it?"
                    ));
                } else {
                    ui.label(format!("Delete \u{201c}{name}\u{201d}?"));
                }
                ui.colored_label(ui.visuals().warn_fg_color, "This can't be undone.");
                if let Some(err) = &self.delete_error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = 2;
                    }
                    let btn = egui::Button::new(egui::RichText::new("Delete").strong())
                        .fill(egui::Color32::from_rgb(170, 60, 60));
                    if ui.add(btn).clicked() {
                        action = 1;
                    }
                });
            });

        match action {
            1 => match self.delete_path(&path, is_dir) {
                Ok(()) => {
                    self.pending_delete = None;
                    self.delete_error = None;
                }
                Err(e) => self.delete_error = Some(e),
            },
            2 => {
                self.pending_delete = None;
                self.delete_error = None;
            }
            _ => {}
        }
    }

    /// Remove a file/folder from disk and close any editor tabs/buffers for it.
    fn delete_path(&mut self, path: &Path, is_dir: bool) -> Result<(), String> {
        if is_dir {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        // Forget any open buffers / editor tabs for the path (or its children).
        self.open_files
            .retain(|p, _| !(p == path || p.starts_with(path)));
        self.editor_open.retain(|it| match it {
            EditorItem::File(p) => !(p == path || p.starts_with(path)),
            _ => true,
        });
        if self.editor_active >= self.editor_open.len() {
            self.editor_active = self.editor_open.len().saturating_sub(1);
        }
        // Nudge the git/tree state to refresh now that the tree changed.
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }

    pub(crate) fn focus_or_open(&mut self, item: EditorItem) {
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    /// The inline blame gutter: each source line annotated with the commit that
    /// last touched it, syntax-highlighted, shown in place of the editor.
    /// Read-only and self-consistent (line text comes from `git blame`).
    pub(crate) fn render_blame_view(&self, ui: &mut egui::Ui, path: &Path, lang: &str) {
        let Some(lines) = self.blame_cache.get(path) else {
            ui.weak("No blame data.");
            return;
        };
        if lines.is_empty() {
            ui.weak("No blame data (file not tracked, or git unavailable).");
            return;
        }
        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
        let hash_color = egui::Color32::from_rgb(160, 140, 200);
        let meta_color = egui::Color32::from_gray(135);
        let num_color = egui::Color32::from_gray(100);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let total = lines.len();
        let num_w = total.to_string().len();

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("blame-inline", path))
            .show_rows(ui, row_h, total, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in range {
                    let bl = &lines[i];
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(
                            egui::RichText::new(format!("{:>8}", bl.short))
                                .monospace()
                                .color(hash_color),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{:<12} {}",
                                truncate(&bl.author, 12),
                                bl.date
                            ))
                            .monospace()
                            .color(meta_color),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:>w$}", i + 1, w = num_w))
                                .monospace()
                                .color(num_color),
                        );
                        let mut job = egui_extras::syntax_highlighting::highlight(
                            ui.ctx(),
                            ui.style(),
                            &theme,
                            &bl.line,
                            lang,
                        );
                        job.wrap.max_width = f32::INFINITY;
                        ui.add(
                            egui::Label::new(job)
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
                }
            });
    }

    /// An editable file tab — or, while blame is toggled on, the inline blame view.
    pub fn render_file(&mut self, ui: &mut egui::Ui, path: &Path) {
        let git_repo = self.git_repo;
        if !self.open_files.contains_key(path) {
            ui.centered_and_justified(|ui| {
                ui.weak("file not open");
            });
            return;
        }
        let blame_on = self.blame_files.contains(path);
        let dirty = self.open_files.get(path).map(|b| b.dirty).unwrap_or(false);
        let load_error = self.open_files.get(path).and_then(|b| b.load_error.clone());
        let save_error = self.open_files.get(path).and_then(|b| b.save_error.clone());

        let mut do_save = false;
        let mut do_blame = false;
        egui::Panel::top(egui::Id::new(("file-bar", path))).show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(path.display().to_string())
                        .monospace()
                        .small(),
                );
                if dirty {
                    ui.colored_label(egui::Color32::from_rgb(220, 190, 110), "● unsaved");
                }
                if blame_on && dirty {
                    ui.label(egui::RichText::new("blame = saved file").weak().small());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !blame_on && ui.button("💾 Save").clicked() {
                        do_save = true;
                    }
                    if git_repo
                        && ui
                            .selectable_label(blame_on, "🔍 Blame")
                            .on_hover_text("Toggle inline git blame on this file")
                            .clicked()
                    {
                        do_blame = true;
                    }
                });
            });
            ui.add_space(2.0);
        });

        if do_blame {
            self.toggle_blame(path);
        }
        let blame_on = self.blame_files.contains(path);

        if let Some(err) = &load_error {
            ui.colored_label(
                egui::Color32::from_rgb(240, 130, 130),
                format!("Cannot open file: {err}"),
            );
            return;
        }
        if let Some(err) = &save_error {
            ui.colored_label(
                egui::Color32::from_rgb(240, 130, 130),
                format!("Save failed: {err}"),
            );
        }

        let lang = language_for(path);

        if blame_on {
            self.render_blame_view(ui, path, lang);
            return;
        }

        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(ui.ctx(), ui.style());
        let Some(buf) = self.open_files.get_mut(path) else {
            return;
        };
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt(("file-scroll", path))
            .show(ui, |ui| {
                let mut layouter = |lui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
                    let mut job = egui_extras::syntax_highlighting::highlight(
                        lui.ctx(),
                        lui.style(),
                        &theme,
                        text.as_str(),
                        lang,
                    );
                    job.wrap.max_width = f32::INFINITY; // no wrap; horizontal scroll
                    lui.fonts_mut(|f| f.layout_job(job))
                };
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut buf.content)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(40)
                        .layouter(&mut layouter),
                );
                if resp.changed() {
                    buf.dirty = true;
                }
            });

        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            do_save = true;
        }
        if do_save {
            match std::fs::write(path, buf.content.as_bytes()) {
                Ok(()) => {
                    buf.dirty = false;
                    buf.save_error = None;
                }
                Err(e) => buf.save_error = Some(e.to_string()),
            }
        }
    }
}

/// Map a file extension to a syntect language token for highlighting.
pub(crate) fn language_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rs",
        "py" | "pyw" => "py",
        "toml" => "toml",
        "json" => "json",
        "md" | "markdown" => "md",
        "js" | "mjs" | "cjs" => "js",
        "ts" | "tsx" => "ts",
        "html" | "htm" => "html",
        "css" => "css",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
        "go" => "go",
        "java" => "java",
        "yaml" | "yml" => "yaml",
        "xml" => "xml",
        "sql" => "sql",
        "rb" => "rb",
        "php" => "php",
        "lua" => "lua",
        _ => "txt",
    }
}

#[cfg(test)]
mod tests {
    use super::language_for;
    use std::path::Path;

    #[test]
    fn language_for_known_extensions() {
        assert_eq!(language_for(Path::new("src/main.rs")), "rs");
        assert_eq!(language_for(Path::new("a.py")), "py");
        assert_eq!(language_for(Path::new("Cargo.toml")), "toml");
        assert_eq!(language_for(Path::new("README.md")), "md");
    }

    #[test]
    fn language_for_unknown_is_txt() {
        assert_eq!(language_for(Path::new("file.xyz")), "txt");
        assert_eq!(language_for(Path::new("noext")), "txt");
    }
}
