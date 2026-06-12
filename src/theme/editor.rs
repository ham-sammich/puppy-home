//! The live theme editor window: edit the UI palette (with a library of named,
//! saved custom themes) and the terminal palette. Persistence (themes.json /
//! terminal.json) is handled here so the app layer stays thin.

use eframe::egui;

use super::terminal::{TerminalTheme, save_terminal};
use super::{ThemePalette, parse_hex, save_themes, to_hex};

/// What the editor wants the app to do after a frame.
#[derive(Default)]
pub struct EditorOutcome {
    /// The UI palette buffer changed — live-apply `palette.to_visuals()`.
    pub changed: bool,
    /// Make `Theme::Custom(this name)` the active selection.
    pub select: Option<String>,
}

/// Friendly labels for the 16 base ANSI slots.
const ANSI_NAMES: [&str; 16] = [
    "0 black",
    "1 red",
    "2 green",
    "3 yellow",
    "4 blue",
    "5 magenta",
    "6 cyan",
    "7 white",
    "8 br-black",
    "9 br-red",
    "10 br-green",
    "11 br-yellow",
    "12 br-blue",
    "13 br-magenta",
    "14 br-cyan",
    "15 br-white",
];

/// One labeled color-picker row (swatch + editable hex). Returns true if changed.
fn color_row(ui: &mut egui::Ui, label: &str, hex: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut c = parse_hex(hex).unwrap_or(egui::Color32::GRAY);
        if ui.color_edit_button_srgba(&mut c).changed() {
            *hex = to_hex(c);
            changed = true;
        }
        let resp = ui.add(
            egui::TextEdit::singleline(hex)
                .desired_width(78.0)
                .font(egui::TextStyle::Monospace),
        );
        if resp.changed() {
            changed = true;
        }
        ui.label(label);
    });
    changed
}

/// The theme editor window. Mutates the buffers/library in place and persists on
/// the Save buttons; the returned [`EditorOutcome`] drives live preview.
pub fn editor_window(
    ctx: &egui::Context,
    open: &mut bool,
    palette: &mut ThemePalette,
    library: &mut Vec<ThemePalette>,
    term: &mut TerminalTheme,
) -> EditorOutcome {
    let mut out = EditorOutcome::default();
    egui::Window::new("Theme editor")
        .open(open)
        .resizable(true)
        .default_width(360.0)
        .default_height(520.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui_palette_section(ui, palette, library, &mut out);
                ui.add_space(8.0);
                ui.separator();
                terminal_section(ui, term);
            });
        });
    out
}

/// The UI-palette half: saved-theme library + the color rows.
fn ui_palette_section(
    ui: &mut egui::Ui,
    palette: &mut ThemePalette,
    library: &mut Vec<ThemePalette>,
    out: &mut EditorOutcome,
) {
    ui.heading("UI theme");

    // Saved-theme library: load / save / delete.
    ui.horizontal(|ui| {
        ui.label("Saved:");
        egui::ComboBox::from_id_salt("saved-themes")
            .selected_text(if library.is_empty() {
                "(none yet)".to_string()
            } else {
                palette.name.clone()
            })
            .show_ui(ui, |ui| {
                for t in library.iter() {
                    if ui
                        .selectable_label(t.name == palette.name, &t.name)
                        .clicked()
                    {
                        *palette = t.clone();
                        out.changed = true;
                        out.select = Some(palette.name.clone());
                    }
                }
            });
        if ui.button("New").clicked() {
            let mut p = ThemePalette::dark();
            p.name = unique_name("My theme", library);
            *palette = p;
            out.changed = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.text_edit_singleline(&mut palette.name);
        if ui.checkbox(&mut palette.dark_mode, "dark base").changed() {
            out.changed = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("Start from:");
        if ui.button("Dark").clicked() {
            let name = palette.name.clone();
            *palette = ThemePalette::dark();
            palette.name = name;
            out.changed = true;
        }
        if ui.button("Light").clicked() {
            let name = palette.name.clone();
            *palette = ThemePalette::light();
            palette.name = name;
            out.changed = true;
        }
    });

    ui.add_space(4.0);
    let mut any = false;
    any |= color_row(ui, "Text", &mut palette.text);
    any |= color_row(ui, "Weak text", &mut palette.weak_text);
    any |= color_row(ui, "Strong text", &mut palette.strong_text);
    any |= color_row(ui, "Accent", &mut palette.accent);
    any |= color_row(ui, "Selection", &mut palette.selection);
    ui.add_space(2.0);
    any |= color_row(ui, "Panel bg", &mut palette.panel);
    any |= color_row(ui, "Window bg", &mut palette.window);
    any |= color_row(ui, "Faint bg", &mut palette.faint_bg);
    any |= color_row(ui, "Extreme bg", &mut palette.extreme_bg);
    any |= color_row(ui, "Code bg", &mut palette.code_bg);
    ui.add_space(2.0);
    any |= color_row(ui, "Widget bg", &mut palette.widget_bg);
    any |= color_row(ui, "Widget hover", &mut palette.widget_hover);
    any |= color_row(ui, "Widget active", &mut palette.widget_active);
    any |= color_row(ui, "Stroke", &mut palette.stroke);
    ui.add_space(2.0);
    any |= color_row(ui, "Warn", &mut palette.warn);
    any |= color_row(ui, "Error", &mut palette.error);
    ui.add_space(2.0);
    // Redesign tokens: brand accents + per-state agent-card colors.
    any |= color_row(ui, "Accent 2", &mut palette.accent2);
    any |= color_row(ui, "Accent ink", &mut palette.accent_ink);
    any |= color_row(ui, "Status: run", &mut palette.status_run);
    any |= color_row(ui, "Status: think", &mut palette.status_think);
    any |= color_row(ui, "Status: wait", &mut palette.status_wait);
    any |= color_row(ui, "Status: paused", &mut palette.status_paused);
    any |= color_row(ui, "Status: error", &mut palette.status_error);
    if any {
        out.changed = true;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let named = !palette.name.trim().is_empty();
        if ui
            .add_enabled(named, egui::Button::new("Save theme"))
            .on_hover_text("Add/update this theme in themes.json")
            .clicked()
        {
            upsert(library, palette.clone());
            save_themes(library);
            out.select = Some(palette.name.clone());
            out.changed = true;
        }
        let exists = library.iter().any(|t| t.name == palette.name);
        if ui
            .add_enabled(exists, egui::Button::new("Delete"))
            .clicked()
        {
            library.retain(|t| t.name != palette.name);
            save_themes(library);
        }
    });
    if let Some(p) = super::themes_path() {
        ui.label(egui::RichText::new(p.display().to_string()).weak().small());
    }
}

/// The terminal-palette half: fg/bg/cursor + the 16 ANSI colors.
fn terminal_section(ui: &mut egui::Ui, term: &mut TerminalTheme) {
    ui.heading("Terminal theme");
    ui.horizontal(|ui| {
        ui.label("Start from:");
        if ui.button("Dark").clicked() {
            *term = TerminalTheme::dark();
        }
        if ui.button("Light").clicked() {
            *term = TerminalTheme::light();
        }
    });
    ui.add_space(4.0);
    color_row(ui, "Foreground", &mut term.fg);
    color_row(ui, "Background", &mut term.bg);
    color_row(ui, "Cursor", &mut term.cursor);
    ui.add_space(4.0);
    ui.label(egui::RichText::new("ANSI palette").strong());
    if term.ansi.len() < 16 {
        term.ansi.resize(16, "#888888".to_string());
    }
    for (i, name) in ANSI_NAMES.iter().enumerate() {
        color_row(ui, name, &mut term.ansi[i]);
    }
    ui.add_space(6.0);
    if ui
        .button("Save terminal")
        .on_hover_text("Write terminal.json")
        .clicked()
    {
        save_terminal(term);
    }
    if let Some(p) = super::config_path("terminal.json") {
        ui.label(egui::RichText::new(p.display().to_string()).weak().small());
    }
}

/// Insert or replace a theme in the library, keyed by name.
fn upsert(library: &mut Vec<ThemePalette>, theme: ThemePalette) {
    if let Some(slot) = library.iter_mut().find(|t| t.name == theme.name) {
        *slot = theme;
    } else {
        library.push(theme);
    }
}

/// A name not already taken in the library (`base`, `base 2`, `base 3`, …).
fn unique_name(base: &str, library: &[ThemePalette]) -> String {
    if !library.iter().any(|t| t.name == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|cand| !library.iter().any(|t| &t.name == cand))
        .unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_by_name() {
        let mut lib = vec![ThemePalette::dark()];
        lib[0].name = "X".into();
        let mut updated = ThemePalette::light();
        updated.name = "X".into();
        upsert(&mut lib, updated);
        assert_eq!(lib.len(), 1);
        assert!(!lib[0].dark_mode); // replaced with the light-based one
    }

    #[test]
    fn unique_name_avoids_collisions() {
        let mut a = ThemePalette::dark();
        a.name = "My theme".into();
        let lib = vec![a];
        assert_eq!(unique_name("My theme", &lib), "My theme 2");
        assert_eq!(unique_name("Fresh", &lib), "Fresh");
    }
}
