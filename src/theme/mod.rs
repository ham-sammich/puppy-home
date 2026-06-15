//! Theming: editable color palettes, built-in presets (Dark / Light), a library
//! of named custom themes on disk, and the terminal palette.
//!
//! A [`ThemePalette`] is a flat bag of `#rrggbb` hex strings so it can be
//! hand-edited in plain JSON *and* tweaked with color pickers — the same data
//! drives the GPUI shell, which parses the hex into its own color type.

mod library;
mod terminal;

pub use library::{ANSI_NAMES, unique_name, upsert};
pub use terminal::save_terminal;
pub use terminal::{TerminalTheme, load_terminal};

use std::path::PathBuf;

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
    /// App backdrop behind all panels (GPUI shell's outermost fill; egui's
    /// outermost surface is `panel`, so its renderer ignores this).
    #[serde(default = "d_app_bg")]
    pub app_bg: String,
    /// Dimmest text tier (idle labels; below `weak_text`). GPUI shell.
    #[serde(default = "d_dim_text")]
    pub dim_text: String,
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
fn d_app_bg() -> String {
    "#121217".into()
}
fn d_dim_text() -> String {
    "#696977".into()
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self::dark()
    }
}

/// Parse an `#rrggbb` (or `rrggbb`) hex string into an `(r, g, b)` triple.
/// Returns `None` if malformed. Frontend-agnostic: the GPUI shell maps the
/// triple onto its own color type ([`gpui_ui::tokens::hex`]).
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

impl ThemePalette {
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
            app_bg: d_app_bg(),
            dim_text: d_dim_text(),
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
            app_bg: "#e9e9ee".into(),
            dim_text: "#8a8a96".into(),
        }
    }
}

/// Resolve the palette for a selection, looking custom themes up by name
/// in `library`. An unknown custom name falls back to Dark. The GPUI shell
/// maps the result onto its `Tokens`.
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
    fn hex_parses_to_rgb_tuple() {
        assert_eq!(parse_hex("#1ea53c"), Some((0x1e, 0xa5, 0x3c)));
        assert_eq!(parse_hex("1ea53c"), Some((0x1e, 0xa5, 0x3c)));
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
                &p.app_bg,
                &p.dim_text,
            ] {
                assert!(parse_hex(hex).is_some(), "bad hex {hex:?} in {}", p.name);
            }
        }
    }

    #[test]
    fn light_weak_text_is_dark_enough_to_read() {
        // Regression guard for the reported bug: weak text on white must not be
        // washed out. Require the weak-text luminance clearly below white.
        let (r, g, b) = parse_hex(&ThemePalette::light().weak_text).unwrap();
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        assert!(lum < 150.0, "light weak-text too bright: {lum}");
    }

    #[test]
    fn palette_for_resolves_presets_customs_and_fallback() {
        let mut neon = ThemePalette::dark();
        neon.name = "Neon".into();
        neon.accent = "#00ff88".into();
        let lib = vec![neon];
        assert_eq!(palette_for(&Theme::Dark, &lib).accent, "#e7ab4d");
        assert!(!palette_for(&Theme::Light, &lib).dark_mode);
        assert_eq!(
            palette_for(&Theme::Custom("Neon".into()), &lib).accent,
            "#00ff88"
        );
        // Unknown custom name falls back to Dark, not a crash.
        assert_eq!(
            palette_for(&Theme::Custom("gone".into()), &lib).accent,
            "#e7ab4d"
        );
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
        assert_eq!(p.app_bg, "#121217"); // GPUI backdrop default
        assert_eq!(p.dim_text, "#696977");
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
