//! A small reusable directory/file browser body.
//!
//! Renders a directory listing with folder navigation + a pick action. The
//! caller owns *how* it lists (local fs, remote fs, or SSH `ls`) and the modal
//! window; this just draws the entries and reports what the user clicked, so the
//! workspace file picker and the remote-connect folder picker share one UI.

use eframe::egui;

/// What the user asked for this frame while browsing.
pub enum BrowseAction {
    /// Descend into a subfolder (by name, relative to the shown dir).
    Enter(String),
    /// Go to the parent directory.
    Up,
    /// Pick the current directory (`None`) or a file in it (`Some(name)`).
    Pick(Option<String>),
}

/// Draw the path header, an optional error, and the entry list. `entries` is
/// `(name, is_dir)`. When `pick_dir` is true only folders are shown (and the
/// current directory is the pickable thing); otherwise files are pickable too.
pub fn render_listing(
    ui: &mut egui::Ui,
    cwd: &str,
    entries: &[(String, bool)],
    pick_dir: bool,
    loading: bool,
    error: Option<&str>,
) -> Option<BrowseAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        if ui.button(".. up").on_hover_text("Parent folder").clicked() {
            action = Some(BrowseAction::Up);
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(cwd).monospace().weak());
        if loading {
            ui.spinner();
        }
    });
    ui.separator();
    if let Some(err) = error {
        ui.colored_label(ui.visuals().error_fg_color, err);
    }

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Folders first, then (in file mode) files; each list alphabetical.
            let mut dirs: Vec<&str> = entries
                .iter()
                .filter(|(_, is_dir)| *is_dir)
                .map(|(n, _)| n.as_str())
                .collect();
            dirs.sort_unstable();
            for name in &dirs {
                if ui.button(format!("{name}/")).clicked() {
                    action = Some(BrowseAction::Enter((*name).to_string()));
                }
            }

            if !pick_dir {
                let mut files: Vec<&str> = entries
                    .iter()
                    .filter(|(_, is_dir)| !*is_dir)
                    .map(|(n, _)| n.as_str())
                    .collect();
                files.sort_unstable();
                for name in &files {
                    if ui
                        .add(egui::Button::new(name.to_string()).fill(egui::Color32::TRANSPARENT))
                        .clicked()
                    {
                        action = Some(BrowseAction::Pick(Some((*name).to_string())));
                    }
                }
            }

            if entries.is_empty() && !loading {
                ui.weak("(empty)");
            }
        });

    if pick_dir {
        ui.separator();
        if ui
            .button("Use this folder")
            .on_hover_text("Open this directory in the workspace")
            .clicked()
        {
            action = Some(BrowseAction::Pick(None));
        }
    }

    action
}
