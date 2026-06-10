//! Theming: editable color palettes, built-in presets (Dark / Light), a library
//! of named custom themes on disk, the terminal palette, and a live GUI editor.
//!
//! A [`ThemePalette`] is a flat bag of `#rrggbb` hex strings so it can be
//! hand-edited in plain JSON *and* tweaked with color pickers — the same data
//! drives both. [`ThemePalette::to_visuals`] is the single palette to egui
//! `Visuals` bridge (DRY: nobody else pokes at `Visuals` directly).

mod editor;
mod terminal;

pub use editor::editor_window;
pub use terminal::{ResolvedTerminal, TerminalTheme, load_terminal, terminal_colors_id};

use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::session::Theme;

/// A complete, editable UI color palette. Every field is an `#rrggbb` hex string
/// so the on-disk theme files stay human-friendly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemePalette {
    /// Theme name — also its identity in the saved-theme library.
    pub name: String,
    /// Overall light/dark base (egui uses it for a few derived defaults).
    pub dark_mode: bool,
    /// Primary text color.
    pub text: String,
    /// Dimmed/secondary text (`ui.weak`). The light-mode contrast fix lives here.
    pub weak_text: String,
    /// Emphasized text (`RichText::strong`, headings).
    pub strong_text: String,
    /// Panel background.
    pub panel: String,
    /// Window/popup background.
    pub window: String,
    /// Barely-different background (striped grids, zebra rows).
    pub faint_bg: String,
    /// Text-edit / scrollbar background.
    pub extreme_bg: String,
    /// Background behind code-styled monospaced labels.
    pub code_bg: String,
    /// Accent: hyperlinks + general highlight.
    pub accent: String,
    /// Selection background.
    pub selection: String,
    /// Button (interactive widget) background at rest.
    pub widget_bg: String,
    /// Button background when hovered.
    pub widget_hover: String,
    /// Button background when pressed/active.
    pub widget_active: String,
    /// Outlines/separators (widget strokes).
    pub stroke: String,
    /// Warning text.
    pub warn: String,
    /// Error text.
    pub error: String,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self::dark()
    }
}

/// Parse an `#rrggbb` (or `rrggbb`) hex string. Returns `None` if malformed.
pub fn parse_hex(s: &str) -> Option<egui::Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

/// Format a color as `#rrggbb`.
pub fn to_hex(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

impl ThemePalette {
    /// A hex field as a `Color32`, falling back to mid-gray if hand-edited badly
    /// (a typo in a theme file should never crash the app).
    fn col(&self, hex: &str) -> egui::Color32 {
        parse_hex(hex).unwrap_or(egui::Color32::GRAY)
    }

    /// Map this palette onto an egui `Visuals`. The *only* palette to egui bridge.
    pub fn to_visuals(&self) -> egui::Visuals {
        let mut v = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        let text = self.col(&self.text);
        let strong = self.col(&self.strong_text);
        let stroke = egui::Stroke::new(1.0, self.col(&self.stroke));

        v.dark_mode = self.dark_mode;
        v.weak_text_color = Some(self.col(&self.weak_text));
        v.panel_fill = self.col(&self.panel);
        v.window_fill = self.col(&self.window);
        v.window_stroke = stroke;
        v.faint_bg_color = self.col(&self.faint_bg);
        v.extreme_bg_color = self.col(&self.extreme_bg);
        v.code_bg_color = self.col(&self.code_bg);
        v.hyperlink_color = self.col(&self.accent);
        v.warn_fg_color = self.col(&self.warn);
        v.error_fg_color = self.col(&self.error);
        v.selection.bg_fill = self.col(&self.selection);
        v.selection.stroke = egui::Stroke::new(1.0, text);

        // Widget states: text everywhere, distinct fills per interaction state.
        let w = &mut v.widgets;
        for ws in [&mut w.noninteractive, &mut w.inactive, &mut w.open] {
            ws.fg_stroke.color = text;
            ws.bg_stroke = stroke;
        }
        w.hovered.fg_stroke.color = text;
        w.active.fg_stroke.color = strong; // strong/heading text reads from here
        w.noninteractive.bg_fill = self.col(&self.panel);
        w.inactive.weak_bg_fill = self.col(&self.widget_bg);
        w.inactive.bg_fill = self.col(&self.widget_bg);
        w.hovered.weak_bg_fill = self.col(&self.widget_hover);
        w.hovered.bg_fill = self.col(&self.widget_hover);
        w.hovered.bg_stroke = stroke;
        w.active.weak_bg_fill = self.col(&self.widget_active);
        w.active.bg_fill = self.col(&self.widget_active);
        w.active.bg_stroke = egui::Stroke::new(1.0, strong);
        w.open.weak_bg_fill = self.col(&self.widget_hover);
        v
    }

    /// The default dark preset (comfortable, neutral).
    pub fn dark() -> Self {
        Self {
            name: "Dark".into(),
            dark_mode: true,
            text: "#e6e6e6".into(),
            weak_text: "#9aa0aa".into(),
            strong_text: "#ffffff".into(),
            panel: "#1e1e24".into(),
            window: "#26262e".into(),
            faint_bg: "#2a2a32".into(),
            extreme_bg: "#16161a".into(),
            code_bg: "#2a2a32".into(),
            accent: "#5a9cff".into(),
            selection: "#365880".into(),
            widget_bg: "#33333d".into(),
            widget_hover: "#404049".into(),
            widget_active: "#4a4a56".into(),
            stroke: "#45454f".into(),
            warn: "#e8c06a".into(),
            error: "#f08080".into(),
        }
    }

    /// The light preset — tuned so "weak" text stays readable on white (the
    /// reported contrast bug: light-gray-on-white was unreadable).
    pub fn light() -> Self {
        Self {
            name: "Light".into(),
            dark_mode: false,
            text: "#1d1f23".into(),
            weak_text: "#5b616b".into(), // medium gray, not the washed-out default
            strong_text: "#0b0c0e".into(),
            panel: "#f4f4f6".into(),
            window: "#ffffff".into(),
            faint_bg: "#e9e9ee".into(),
            extreme_bg: "#ffffff".into(),
            code_bg: "#ececf1".into(),
            accent: "#1b6feb".into(),
            selection: "#aacbff".into(),
            widget_bg: "#e4e4ea".into(),
            widget_hover: "#d6d6df".into(),
            widget_active: "#c4c4d0".into(),
            stroke: "#bfbfca".into(),
            warn: "#9a6b00".into(),
            error: "#b3261e".into(),
        }
    }
}

/// Resolve the egui visuals for a selection, looking custom themes up by name in
/// `library`. An unknown custom name falls back to Dark.
pub fn visuals_for(theme: &Theme, library: &[ThemePalette]) -> egui::Visuals {
    match theme {
        Theme::Dark => ThemePalette::dark().to_visuals(),
        Theme::Light => ThemePalette::light().to_visuals(),
        Theme::Custom(name) => library
            .iter()
            .find(|p| &p.name == name)
            .map(ThemePalette::to_visuals)
            .unwrap_or_else(|| ThemePalette::dark().to_visuals()),
    }
}

/// Per-OS config path for a puppy-home file: `<config>/puppy-home/<file>`.
pub(crate) fn config_path(file: &str) -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("puppy-home").join(file))
}

/// Path to the saved custom-theme library (`themes.json`).
pub fn themes_path() -> Option<PathBuf> {
    config_path("themes.json")
}

/// Load the saved custom themes (empty if missing/unreadable).
pub fn load_themes() -> Vec<ThemePalette> {
    let Some(path) = themes_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Write the custom-theme library to `themes.json` (best-effort).
pub fn save_themes(themes: &[ThemePalette]) {
    let Some(path) = themes_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(themes) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrips() {
        let c = egui::Color32::from_rgb(0x1e, 0xa5, 0x3c);
        assert_eq!(to_hex(c), "#1ea53c");
        assert_eq!(parse_hex("#1ea53c"), Some(c));
        assert_eq!(parse_hex("1ea53c"), Some(c));
    }

    #[test]
    fn parse_hex_rejects_garbage() {
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn presets_have_valid_colors() {
        for p in [ThemePalette::dark(), ThemePalette::light()] {
            for hex in [
                &p.text, &p.weak_text, &p.strong_text, &p.panel, &p.window, &p.faint_bg,
                &p.extreme_bg, &p.code_bg, &p.accent, &p.selection, &p.widget_bg, &p.widget_hover,
                &p.widget_active, &p.stroke, &p.warn, &p.error,
            ] {
                assert!(parse_hex(hex).is_some(), "bad hex {hex:?} in {}", p.name);
            }
        }
    }

    #[test]
    fn light_weak_text_is_dark_enough_to_read() {
        // Regression guard for the reported bug: weak text on white must not be
        // washed out. Require the weak-text luminance clearly below white.
        let c = parse_hex(&ThemePalette::light().weak_text).unwrap();
        let lum = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        assert!(lum < 150.0, "light weak-text too bright: {lum}");
    }

    #[test]
    fn custom_resolves_by_name() {
        let mut p = ThemePalette::dark();
        p.name = "Neon".into();
        p.panel = "#100020".into();
        let lib = vec![p];
        let v = visuals_for(&Theme::Custom("Neon".into()), &lib);
        assert_eq!(v.panel_fill, parse_hex("#100020").unwrap());
        // Unknown name falls back to dark, not a crash.
        let v2 = visuals_for(&Theme::Custom("missing".into()), &lib);
        assert_eq!(v2.panel_fill, parse_hex("#1e1e24").unwrap());
    }
}
