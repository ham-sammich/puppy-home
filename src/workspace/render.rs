//! Transcript-entry rendering, markdown, and the file-tree renderer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::backend::BackendMessage;

use super::diff::marker_color;
use super::fs::WorkspaceFs;
use super::state::{Entry, TREE_IGNORE};

pub(crate) const AGENT_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 220, 150);

pub(crate) fn render_markdown(ui: &mut egui::Ui, cache: &mut CommonMarkCache, text: &str) {
    CommonMarkViewer::new().show(ui, cache, text);
}

pub(crate) fn render_entry(
    ui: &mut egui::Ui,
    entry: &Entry,
    cache: &mut CommonMarkCache,
    puppy: &str,
) {
    match entry {
        Entry::User(text) => labelled(ui, "you", egui::Color32::from_rgb(120, 170, 255), text),
        Entry::Agent(text) => {
            ui.colored_label(AGENT_COLOR, format!("🐶 {puppy}:"));
            render_markdown(ui, cache, text);
            ui.add_space(6.0);
        }
        Entry::Note(text) => {
            ui.label(egui::RichText::new(text).weak().italics());
            ui.add_space(4.0);
        }
        Entry::Error(text) => {
            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), format!("⚠ {text}"));
            ui.add_space(4.0);
        }
        Entry::Message(msg) => render_message(ui, msg, cache, puppy),
        Entry::Thinking { text, collapse } => {
            let dim = egui::Color32::from_gray(150);
            let id = ui.id().with("think");
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                true,
            );
            // One-shot fold when the turn finished.
            if collapse.get() {
                state.set_open(false);
                collapse.set(false);
            }
            state
                .show_header(ui, |ui| {
                    ui.label(egui::RichText::new("💭 thinking…").italics().color(dim));
                })
                .body(|ui| {
                    ui.label(egui::RichText::new(text).italics().color(dim));
                });
            ui.add_space(4.0);
        }
    }
}

pub(crate) fn render_message(
    ui: &mut egui::Ui,
    msg: &BackendMessage,
    cache: &mut CommonMarkCache,
    puppy: &str,
) {
    // Agent prose is markdown — render it formatted.
    if msg.category == "agent" {
        ui.label(
            egui::RichText::new(format!("🐶 {puppy}"))
                .color(AGENT_COLOR)
                .small(),
        );
        render_markdown(ui, cache, &msg.text);
        ui.add_space(2.0);
        return;
    }
    let color = match msg.category.as_str() {
        "tool_output" => egui::Color32::from_rgb(200, 180, 120),
        "user_interaction" => egui::Color32::from_rgb(220, 160, 220),
        "divider" => egui::Color32::DARK_GRAY,
        _ => egui::Color32::GRAY,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("[{}]", msg.kind))
                .color(color)
                .small(),
        );
        // Tool output can be enormous (multi-KB JSON dumps); clamp what we
        // render so a single message can't wreck layout or framerate.
        const MAX_CHARS: usize = 4000;
        if msg.text.len() > MAX_CHARS {
            let cut: String = msg.text.chars().take(MAX_CHARS).collect();
            let omitted = msg.text.len().saturating_sub(cut.len());
            ui.label(cut);
            ui.label(
                egui::RichText::new(format!("… (+{omitted} bytes trimmed)"))
                    .weak()
                    .small(),
            );
        } else {
            ui.label(&msg.text);
        }
    });
    ui.add_space(2.0);
}

pub(crate) fn labelled(ui: &mut egui::Ui, who: &str, color: egui::Color32, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color, format!("{who}:"));
        ui.label(text);
    });
    ui.add_space(6.0);
}

/// Actions requested from the file-tree context menus this frame.
#[derive(Default)]
pub(crate) struct TreeActions {
    /// Open this file in the editor.
    pub(crate) open: Option<PathBuf>,
    /// Delete this path (file or folder).
    pub(crate) delete: Option<PathBuf>,
    /// Rename this path.
    pub(crate) rename: Option<PathBuf>,
    /// Create a new entry inside this dir; bool = is-folder.
    pub(crate) new_in: Option<(PathBuf, bool)>,
}

/// Recursively render a directory as a lazy collapsible tree. Only expanded
/// folders are read (the collapsing body runs only when open).
pub(crate) fn render_dir(
    ui: &mut egui::Ui,
    fs: &dyn WorkspaceFs,
    dir: &Path,
    markers: &HashMap<PathBuf, char>,
    acts: &mut TreeActions,
) {
    let Ok(read) = fs.read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(bool, PathBuf, String)> = read
        .into_iter()
        .map(|e| (e.is_dir, e.path, e.name))
        .filter(|(is_dir, _, name)| {
            !(name.is_empty() || *is_dir && TREE_IGNORE.contains(&name.as_str()))
        })
        .collect();
    // Directories first, then case-insensitive alphabetical.
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });

    for (is_dir, path, name) in entries {
        if is_dir {
            let header = egui::CollapsingHeader::new(format!("📁 {name}"))
                .id_salt(&path)
                .show(ui, |ui| render_dir(ui, fs, &path, markers, acts));
            header.header_response.context_menu(|ui| {
                if ui.button("New file").clicked() {
                    acts.new_in = Some((path.clone(), false));
                    ui.close();
                }
                if ui.button("New folder").clicked() {
                    acts.new_in = Some((path.clone(), true));
                    ui.close();
                }
                ui.separator();
                if ui.button("Rename").clicked() {
                    acts.rename = Some(path.clone());
                    ui.close();
                }
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(path.to_string_lossy().into_owned());
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete folder").clicked() {
                    acts.delete = Some(path.clone());
                    ui.close();
                }
            });
        } else {
            let marker = markers.get(&path).copied();
            let resp = ui
                .horizontal(|ui| {
                    let r = ui.selectable_label(false, format!("📄 {name}"));
                    if let Some(m) = marker {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.colored_label(marker_color(m), m.to_string())
                        });
                    }
                    r
                })
                .inner;
            if resp.clicked() {
                acts.open = Some(path.clone());
            }
            resp.context_menu(|ui| {
                if ui.button("Open").clicked() {
                    acts.open = Some(path.clone());
                    ui.close();
                }
                if ui.button("Rename").clicked() {
                    acts.rename = Some(path.clone());
                    ui.close();
                }
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(path.to_string_lossy().into_owned());
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete").clicked() {
                    acts.delete = Some(path.clone());
                    ui.close();
                }
            });
        }
    }
}

/// Shorten an autosave session name (`auto_session_20260519_174443` → readable).
pub(crate) fn short_session(name: &str) -> String {
    let core = name.strip_prefix("auto_session_").unwrap_or(name);
    // "20260519_174443" → "2026-05-19 17:44"
    if core.len() == 15 && core.as_bytes().get(8) == Some(&b'_') {
        let (d, t) = core.split_at(8);
        format!(
            "{}-{}-{} {}:{}",
            &d[0..4],
            &d[4..6],
            &d[6..8],
            &t[1..3],
            &t[3..5]
        )
    } else {
        core.to_string()
    }
}

/// Truncate to `max` chars (for fixed-width blame columns).
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{short_session, truncate};

    #[test]
    fn short_session_formats_autosave_timestamp() {
        assert_eq!(
            short_session("auto_session_20260519_174443"),
            "2026-05-19 17:44"
        );
    }

    #[test]
    fn short_session_passes_through_other_names() {
        assert_eq!(short_session("my_context"), "my_context");
        assert_eq!(short_session("auto_session_weird"), "weird");
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncate_clips_long_strings_to_max_chars() {
        let out = truncate("hello world", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.starts_with("hell"));
    }
}
