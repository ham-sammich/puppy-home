//! The "add a file to the chat" browser.
//!
//! A modal that browses the workspace's files via [`WorkspaceFs`] -- so it works
//! the same for a local folder or a remote (SSH) one -- and drops an `@path`
//! reference into the composer, complementing typing `@` for completion.

use std::path::Path;

use eframe::egui;

use super::Workspace;
use crate::views::path_browser::{self, BrowseAction};

impl Workspace {
    /// Open the file browser at the workspace root.
    pub(crate) fn open_file_browser(&mut self) {
        self.file_browser = Some(self.root.clone());
    }

    /// The file-browser modal; navigates within the workspace and inserts an
    /// `@relpath` reference on pick. Listing rides the same `WorkspaceFs` the
    /// tree uses (async-cached for remote, so it never blocks the frame).
    pub(crate) fn render_file_browser(&mut self, ctx: &egui::Context) {
        let Some(cwd) = self.file_browser.clone() else {
            return;
        };
        let entries: Vec<(String, bool)> = self
            .fs
            .read_dir(&cwd)
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();

        let mut action = None;
        let mut window_open = true;
        egui::Window::new("Add file to chat")
            .id(egui::Id::new(("file-browser", self.id.0)))
            .collapsible(false)
            .resizable(true)
            .open(&mut window_open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(480.0);
                ui.weak("Pick a file to reference with @ in your message.");
                ui.add_space(4.0);
                let cwd_str = cwd.to_string_lossy();
                action = path_browser::render_listing(ui, &cwd_str, &entries, false, false, None);
            });

        let mut close = !window_open;
        match action {
            Some(BrowseAction::Enter(name)) => self.file_browser = Some(cwd.join(name)),
            Some(BrowseAction::Up) => {
                // Stay within the workspace subtree so @refs remain relative.
                if cwd != self.root
                    && let Some(parent) = cwd.parent()
                {
                    self.file_browser = Some(parent.to_path_buf());
                }
            }
            Some(BrowseAction::Pick(Some(name))) => {
                self.insert_file_reference(&cwd.join(name));
                close = true;
            }
            Some(BrowseAction::Pick(None)) | None => {}
        }
        if close {
            self.file_browser = None;
        }
    }

    /// Append an `@<path>` reference (relative to the workspace root, forward
    /// slashes) to the composer input and focus it.
    fn insert_file_reference(&mut self, path: &Path) {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let token = format!("@{}", rel.to_string_lossy().replace('\\', "/"));
        if !self.input.is_empty() && !self.input.ends_with(' ') {
            self.input.push(' ');
        }
        self.input.push_str(&token);
        self.input.push(' ');
        self.request_input_focus = true;
        self.status_line = format!("Added {token} to your message.");
    }
}
