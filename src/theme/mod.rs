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
    // --- redesign tokens (serde defaults keep pre-redesign themes.json loading) ---
    /// Secondary accent (gradients, hover tints on the brand color).
    #[serde(default = "d_accent2")]
    pub accent2: String,
    /// Text/glyphs sitting ON the accent (e.g. the label in an amber chip).
    #[serde(default = "d_accent_ink")]
    pub accent_ink: String,
    /// Status: generating.
    #[serde(default = "d_status_run")]
    pub status_run: String,
    /// Status: thinking / tool call.
    #[serde(default = "d_status_think")]
    pub status_think: String,
    /// Status: waiting for user input.
    #[serde(default = "d_status_wait")]
    pub status_wait: String,
    /// Status: paused at the PauseController gate.
    #[serde(default = "d_status_paused")]
    pub status_paused: String,
    /// Status: errored / dead.
    #[serde(default = "d_status_error")]
    pub status_error: String,
}

// Serde defaults for the redesign tokens — one source of truth shared by the
// field attributes and both presets.
fn d_accent2() -> String {
    "#f0c987".into()
}
fn d_accent_ink() -> String {
    "#1c1402".into()
}
fn d_status_run() -> String {
    "#5fd190".into()
}
fn d_status_think() -> String {
    "#6aa8ff".into()
}
fn d_status_wait() -> String {
    "#dd9ce6".into()
}
fn d_status_paused() -> String {
    "#cfa54e".into()
}
fn d_status_error() -> String {
    "#f28585".into()
}

/// The redesign's accent/status tokens resolved to `Color32` once — views hold
/// one of these instead of parsing hex per frame. Field names follow
/// EGUI_GUIDE.md's `Accents` recipe.
pub struct Accents {
    pub accent: egui::Color32,
    /// Gradient / hover partner of `accent` (the workspace-chat redesign
    /// consumes it; the dashboard doesn't need it yet).
    #[allow(dead_code)]
    pub accent_2: egui::Color32,
    pub accent_ink: egui::Color32,
    pub run: egui::Color32,
    pub think: egui::Color32,
    pub wait: egui::Color32,
    pub paused: egui::Color32,
    pub error: egui::Color32,
}

impl Accents {
    /// Resolve from a palette. Malformed hex falls back to mid-gray, matching
    /// the rest of the palette's lenient parsing.
    pub fn from_palette(p: &ThemePalette) -> Self {
        Accents {
            accent: p.col(&p.accent),
            accent_2: p.col(&p.accent2),
            accent_ink: p.col(&p.accent_ink),
            run: p.col(&p.status_run),
            think: p.col(&p.status_think),
            wait: p.col(&p.status_wait),
            paused: p.col(&p.status_paused),
            error: p.col(&p.status_error),
        }
    }
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
        // Buttons ride the design token radius (8; cards 13 and pills 999 are
        // applied at their call sites).
        let w = &mut v.widgets;
        for ws in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            ws.corner_radius = egui::CornerRadius::same(8);
        }
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

    /// The default dark preset: the redesign's amber brand on the spec's base
    /// surfaces (EGUI_GUIDE.md §1 maps them onto egui visuals; `window` doubles
    /// as the card fill).
    pub fn dark() -> Self {
        Self {
            name: "Dark".into(),
            dark_mode: true,
            text: "#eaeaf0".into(),
            weak_text: "#9a9aa8".into(),
            strong_text: "#ffffff".into(),
            panel: "#1a1a20".into(),
            window: "#1e1e26".into(),
            faint_bg: "#222229".into(),
            extreme_bg: "#16161c".into(),
            code_bg: "#222229".into(),
            accent: "#e7ab4d".into(),
            selection: "#4d3d20".into(), // amber-tinted, not the old blue
            widget_bg: "#2a2a33".into(),
            widget_hover: "#34343f".into(),
            widget_active: "#3e3e4a".into(),
            stroke: "#2e2e39".into(),
            warn: "#e8c06a".into(),
            error: "#f28585".into(),
            accent2: d_accent2(),
            accent_ink: d_accent_ink(),
            status_run: d_status_run(),
            status_think: d_status_think(),
            status_wait: d_status_wait(),
            status_paused: d_status_paused(),
            status_error: d_status_error(),
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
            accent2: d_accent2(),
            accent_ink: d_accent_ink(),
            status_run: d_status_run(),
            status_think: d_status_think(),
            status_wait: d_status_wait(),
            status_paused: d_status_paused(),
            status_error: d_status_error(),
        }
    }
}

/// Resolve the active palette for a theme selection, looking custom themes up
/// by name in `library`. An unknown custom name falls back to Dark. The single
/// source feeding both `visuals_for` and the views' resolved [`Accents`].
pub fn palette_for(theme: &Theme, library: &[ThemePalette]) -> ThemePalette {
    match theme {
        Theme::Dark => ThemePalette::dark(),
        Theme::Light => ThemePalette::light(),
        Theme::Custom(name) => library
            .iter()
            .find(|p| &p.name == name)
            .cloned()
            .unwrap_or_else(ThemePalette::dark),
    }
}

/// Resolve the egui visuals for a selection (see [`palette_for`]).
pub fn visuals_for(theme: &Theme, library: &[ThemePalette]) -> egui::Visuals {
    palette_for(theme, library).to_visuals()
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
                &p.text,
                &p.weak_text,
                &p.strong_text,
                &p.panel,
                &p.window,
                &p.faint_bg,
                &p.extreme_bg,
                &p.code_bg,
                &p.accent,
                &p.selection,
                &p.widget_bg,
                &p.widget_hover,
                &p.widget_active,
                &p.stroke,
                &p.warn,
                &p.error,
                &p.accent2,
                &p.accent_ink,
                &p.status_run,
                &p.status_think,
                &p.status_wait,
                &p.status_paused,
                &p.status_error,
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
        assert_eq!(v2.panel_fill, parse_hex("#1a1a20").unwrap());
    }

    #[test]
    fn legacy_theme_json_loads_with_token_defaults() {
        // A themes.json written before the redesign has none of the token
        // fields — serde defaults must fill them so old libraries still load.
        let legacy = r##"{
            "name": "Old", "dark_mode": true,
            "text": "#e6e6e6", "weak_text": "#9aa0aa", "strong_text": "#ffffff",
            "panel": "#1e1e24", "window": "#26262e", "faint_bg": "#2a2a32",
            "extreme_bg": "#16161a", "code_bg": "#2a2a32", "accent": "#5a9cff",
            "selection": "#365880", "widget_bg": "#33333d",
            "widget_hover": "#404049", "widget_active": "#4a4a56",
            "stroke": "#45454f", "warn": "#e8c06a", "error": "#f08080"
        }"##;
        let p: ThemePalette = serde_json::from_str(legacy).expect("legacy loads");
        assert_eq!(p.accent, "#5a9cff"); // user's choice untouched
        assert_eq!(p.accent2, "#f0c987");
        assert_eq!(p.accent_ink, "#1c1402");
        assert_eq!(p.status_paused, "#cfa54e");
        // And the resolved accessor parses every default.
        let a = Accents::from_palette(&p);
        assert_eq!(a.paused, parse_hex("#cfa54e").unwrap());
        assert_eq!(a.run, parse_hex("#5fd190").unwrap());
    }

    #[test]
    fn dark_preset_is_amber_branded() {
        // The approved redesign decision: amber is the default dark brand.
        let p = ThemePalette::dark();
        assert_eq!(p.accent, "#e7ab4d");
        assert_eq!(p.panel, "#1a1a20");
        assert_eq!(p.window, "#1e1e26");
    }
}
