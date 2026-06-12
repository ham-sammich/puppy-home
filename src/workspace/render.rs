//! Transcript-entry rendering, markdown, and the file-tree renderer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::backend::BackendMessage;

use super::diff::marker_color;
use super::fs::WorkspaceFs;
use super::state::{Entry, TREE_IGNORE, tool_label};

pub(crate) const AGENT_COLOR: egui::Color32 = egui::Color32::from_rgb(150, 220, 150);

/// Per-frame turn context (built once per frame in `view.rs`, borrowed by
/// every entry — no per-entry allocation).
pub(crate) struct TurnMeta<'a> {
    pub(crate) puppy: &'a str,
    pub(crate) agent: &'a str,
    pub(crate) model: &'a str,
}

pub(crate) fn render_markdown(ui: &mut egui::Ui, cache: &mut CommonMarkCache, text: &str) {
    CommonMarkViewer::new().show(ui, cache, text);
}

/// A transcript turn row: 30px avatar tile + (who line, body) column.
fn turn(
    ui: &mut egui::Ui,
    emoji: &str,
    who: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            p.rect_filled(
                rect,
                egui::CornerRadius::same(8),
                ui.visuals().faint_bg_color,
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                emoji,
                egui::FontId::proportional(15.0),
                ui.visuals().text_color(),
            );
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            who(ui);
            body(ui);
        });
    });
    ui.add_space(8.0);
}

/// The puppy turn's who line: accent name + a weak `agent · model` tag.
fn agent_who(ui: &mut egui::Ui, meta: &TurnMeta) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(meta.puppy)
                .color(AGENT_COLOR)
                .strong()
                .small(),
        );
        if !meta.agent.is_empty() {
            ui.label(
                egui::RichText::new(format!("{} \u{00b7} {}", meta.agent, meta.model))
                    .monospace()
                    .weak()
                    .small(),
            );
        }
    });
}

/// A tool-call chip: `🔧 {tool}` + optional file + optional ✓ +A −D counts.
fn tool_chip(ui: &mut egui::Ui, label: &str, detail: Option<&str>, counts: Option<(usize, usize)>) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(
                egui::RichText::new(format!("\u{1f527} {label}"))
                    .monospace()
                    .small(),
            );
            if let Some(d) = detail {
                ui.label(egui::RichText::new(d).monospace().small().weak());
            }
            if let Some((adds, dels)) = counts {
                ui.label(
                    egui::RichText::new(format!("\u{2713} +{adds}"))
                        .monospace()
                        .small()
                        .color(egui::Color32::from_rgb(120, 200, 130)),
                );
                ui.label(
                    egui::RichText::new(format!("\u{2212}{dels}"))
                        .monospace()
                        .small()
                        .color(egui::Color32::from_rgb(230, 120, 120)),
                );
            }
        });
}

pub(crate) fn render_entry(
    ui: &mut egui::Ui,
    entry: &Entry,
    cache: &mut CommonMarkCache,
    meta: &TurnMeta,
) {
    match entry {
        Entry::User(text) => turn(
            ui,
            &crate::session::avatars().0,
            |ui| {
                ui.label(egui::RichText::new("you").weak().small());
            },
            |ui| {
                ui.label(text);
            },
        ),
        Entry::Agent(text) => turn(
            ui,
            &crate::session::avatars().1,
            |ui| agent_who(ui, meta),
            |ui| render_markdown(ui, cache, text),
        ),
        Entry::Note(text) => {
            ui.label(egui::RichText::new(text).weak().italics());
            ui.add_space(4.0);
        }
        Entry::Error(text) => {
            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), format!("⚠ {text}"));
            ui.add_space(4.0);
        }
        Entry::Message(msg) => render_message(ui, msg, cache, meta),
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

/// Light per-frame pass over a `DiffMessage` payload: path + add/del counts
/// only (the full line list is parsed lazily, inside the opened collapsible).
fn diff_chip_info(msg: &BackendMessage) -> Option<(&str, usize, usize)> {
    let p = &msg.payload;
    let path = p.get("path")?.as_str()?;
    let (mut adds, mut dels) = (0usize, 0usize);
    if let Some(arr) = p.get("diff_lines").and_then(serde_json::Value::as_array) {
        for l in arr {
            match l.get("type").and_then(serde_json::Value::as_str) {
                Some("add") => adds += 1,
                Some("remove") => dels += 1,
                _ => {}
            }
        }
    }
    Some((path, adds, dels))
}

pub(crate) fn render_message(
    ui: &mut egui::Ui,
    msg: &BackendMessage,
    cache: &mut CommonMarkCache,
    meta: &TurnMeta,
) {
    // Agent prose is markdown — render it as a full puppy turn.
    if msg.category == "agent" {
        turn(
            ui,
            &crate::session::avatars().1,
            |ui| agent_who(ui, meta),
            |ui| render_markdown(ui, cache, &msg.text),
        );
        return;
    }
    // Diffs: a tool chip (edit · path · ✓ +A −D) over a collapsed colored
    // body. Full line parsing only happens while the body is open.
    if msg.kind == "DiffMessage"
        && let Some((path, adds, dels)) = diff_chip_info(msg)
    {
        ui.horizontal(|ui| {
            tool_chip(ui, "edit", Some(path), Some((adds, dels)));
        });
        egui::CollapsingHeader::new(egui::RichText::new("view diff").weak().small())
            .id_salt(ui.id().with("diff-body"))
            .show(ui, |ui| {
                if let Some(rec) = super::diff::parse_diff(msg) {
                    super::diff::render_diff_lines(ui, &rec.lines);
                }
            });
        ui.add_space(2.0);
        return;
    }
    // Other tool output: a chip with the friendly tool name + clamped text.
    if msg.category == "tool_output" {
        ui.horizontal_wrapped(|ui| {
            tool_chip(ui, &tool_label(&msg.kind), None, None);
            clamped_text(ui, &msg.text);
        });
        ui.add_space(2.0);
        return;
    }
    let color = match msg.category.as_str() {
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
        clamped_text(ui, &msg.text);
    });
    ui.add_space(2.0);
}

/// Tool output can be enormous (multi-KB JSON dumps); clamp what we render so
/// a single message can't wreck layout or framerate.
fn clamped_text(ui: &mut egui::Ui, text: &str) {
    const MAX_CHARS: usize = 4000;
    if text.len() > MAX_CHARS {
        let cut: String = text.chars().take(MAX_CHARS).collect();
        let omitted = text.len().saturating_sub(cut.len());
        ui.label(cut);
        ui.label(
            egui::RichText::new(format!("… (+{omitted} bytes trimmed)"))
                .weak()
                .small(),
        );
    } else {
        ui.label(text);
    }
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
