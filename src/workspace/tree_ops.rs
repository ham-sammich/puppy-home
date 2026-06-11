//! File-tree mutations (create / rename / delete) and their confirm modals,
//! split out of `editor.rs`. All are `impl Workspace` methods and route their
//! filesystem changes through `self.fs` (the [`WorkspaceFs`](super::fs)).

use std::path::{Path, PathBuf};

use eframe::egui;

use super::Workspace;
use super::state::{EditorItem, PendingNew, PendingRename};

impl Workspace {
    /// Confirm + perform a delete requested from the file-tree context menu.
    pub(crate) fn render_delete_modal(&mut self, ctx: &egui::Context) {
        let Some(path) = self.pending_delete.clone() else {
            return;
        };
        let is_dir = self.fs.is_dir(&path);
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
            self.fs.remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            self.fs.remove_file(path).map_err(|e| e.to_string())?;
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

    /// Begin renaming a tree path (opens the rename modal).
    pub(crate) fn start_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.pending_rename = Some(PendingRename {
            path,
            name,
            error: None,
            focus: true,
        });
    }

    /// Begin creating a new file/folder inside `parent` (opens the new modal).
    pub(crate) fn start_new(&mut self, parent: PathBuf, is_dir: bool) {
        self.pending_new = Some(PendingNew {
            parent,
            is_dir,
            name: String::new(),
            error: None,
            focus: true,
        });
    }

    /// The rename modal: a name field + Rename/Cancel.
    pub(crate) fn render_rename_modal(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.pending_rename.take() else {
            return;
        };
        let mut action = 0u8; // 1 = confirm, 2 = cancel
        egui::Window::new("Rename")
            .id(egui::Id::new(("rename-modal", self.id.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label("New name:");
                let resp = ui.text_edit_singleline(&mut state.name);
                if state.focus {
                    resp.request_focus();
                    state.focus = false;
                }
                if let Some(err) = &state.error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(8.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = 2;
                    }
                    if ui.button("Rename").clicked() {
                        action = 1;
                    }
                });
                if enter {
                    action = 1;
                }
            });
        match action {
            1 => match self.perform_rename(&state.path, &state.name) {
                Ok(()) => self.pending_rename = None,
                Err(e) => {
                    state.error = Some(e);
                    self.pending_rename = Some(state);
                }
            },
            2 => self.pending_rename = None,
            _ => self.pending_rename = Some(state),
        }
    }

    /// The new-file/folder modal: a name field + Create/Cancel.
    pub(crate) fn render_new_modal(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.pending_new.take() else {
            return;
        };
        let kind = if state.is_dir { "folder" } else { "file" };
        let mut action = 0u8;
        egui::Window::new(format!("New {kind}"))
            .id(egui::Id::new(("new-modal", self.id.0)))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(format!("New {kind} name:"));
                let resp = ui.text_edit_singleline(&mut state.name);
                if state.focus {
                    resp.request_focus();
                    state.focus = false;
                }
                if let Some(err) = &state.error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(8.0);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = 2;
                    }
                    if ui.button("Create").clicked() {
                        action = 1;
                    }
                });
                if enter {
                    action = 1;
                }
            });
        match action {
            1 => match self.perform_new(&state.parent, state.is_dir, &state.name) {
                Ok(()) => self.pending_new = None,
                Err(e) => {
                    state.error = Some(e);
                    self.pending_new = Some(state);
                }
            },
            2 => self.pending_new = None,
            _ => self.pending_new = Some(state),
        }
    }

    fn perform_rename(&mut self, path: &Path, new_name: &str) -> Result<(), String> {
        let dest = sibling_path(path.parent(), new_name)?;
        if self.fs.exists(&dest) {
            return Err(format!(
                "\u{201c}{}\u{201d} already exists",
                new_name.trim()
            ));
        }
        self.fs.rename(path, &dest).map_err(|e| e.to_string())?;
        // Keep open buffers / editor tabs pointing at the renamed path(s).
        let taken = std::mem::take(&mut self.open_files);
        for (p, buf) in taken {
            self.open_files.insert(remap(&p, path, &dest), buf);
        }
        for item in &mut self.editor_open {
            if let EditorItem::File(p) = item {
                *p = remap(p, path, &dest);
            }
        }
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }

    fn perform_new(&mut self, parent: &Path, is_dir: bool, name: &str) -> Result<(), String> {
        let dest = sibling_path(Some(parent), name)?;
        if self.fs.exists(&dest) {
            return Err(format!("\u{201c}{}\u{201d} already exists", name.trim()));
        }
        if is_dir {
            self.fs.create_dir(&dest).map_err(|e| e.to_string())?;
        } else {
            self.fs.create_file(&dest).map_err(|e| e.to_string())?;
            self.open_editor_file(dest.clone());
        }
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }
}

/// Validate a bare entry name (no separators) and join it onto `parent`.
fn sibling_path(parent: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name can't be empty".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Name can't contain path separators".into());
    }
    let parent = parent.ok_or("No parent directory")?;
    Ok(parent.join(name))
}

/// Rewrite `p` if it equals `old` or lives under it, mapping that prefix to
/// `new` (used to keep open buffers/tabs valid across a rename).
fn remap(p: &Path, old: &Path, new: &Path) -> PathBuf {
    if p == old {
        new.to_path_buf()
    } else if let Ok(rel) = p.strip_prefix(old) {
        new.join(rel)
    } else {
        p.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::{remap, sibling_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn sibling_path_validates() {
        assert!(sibling_path(Some(Path::new("/a")), "").is_err());
        assert!(sibling_path(Some(Path::new("/a")), "a/b").is_err());
        assert!(sibling_path(Some(Path::new("/a")), "a\\b").is_err());
        assert_eq!(
            sibling_path(Some(Path::new("/a")), " notes.txt ").unwrap(),
            PathBuf::from("/a/notes.txt")
        );
    }

    #[test]
    fn remap_rewrites_old_prefix() {
        let old = Path::new("/a/old");
        let new = Path::new("/a/new");
        assert_eq!(
            remap(Path::new("/a/old"), old, new),
            PathBuf::from("/a/new")
        );
        assert_eq!(
            remap(Path::new("/a/old/x.rs"), old, new),
            PathBuf::from("/a/new/x.rs")
        );
        assert_eq!(
            remap(Path::new("/a/other"), old, new),
            PathBuf::from("/a/other")
        );
    }
}
