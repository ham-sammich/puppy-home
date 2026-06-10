//! The embedded terminal's color theme: foreground/background/cursor + the 16
//! base ANSI colors. Persisted to `terminal.json` and handed to `terminal.rs`
//! each frame via egui's per-context data store (so we don't have to thread it
//! through Supervisor -> Workspace -> Terminal).

use eframe::egui;
use serde::{Deserialize, Serialize};

use super::{config_path, parse_hex};

/// Editable terminal palette (hex strings, file-friendly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTheme {
    pub name: String,
    /// Default foreground (text) color.
    pub fg: String,
    /// Default background color.
    pub bg: String,
    /// Cursor color.
    pub cursor: String,
    /// The 16 base ANSI colors (0-7 normal, 8-15 bright).
    pub ansi: Vec<String>,
}

/// Terminal colors resolved to `Color32`, cached per-frame for the renderer.
#[derive(Debug, Clone)]
pub struct ResolvedTerminal {
    pub fg: egui::Color32,
    pub bg: egui::Color32,
    pub cursor: egui::Color32,
    pub ansi: [egui::Color32; 16],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Default for ResolvedTerminal {
    fn default() -> Self {
        TerminalTheme::dark().resolve()
    }
}

impl TerminalTheme {
    /// The classic dark terminal palette (matches the old hard-coded values).
    pub fn dark() -> Self {
        Self {
            name: "Dark".into(),
            fg: "#cccccc".into(),
            bg: "#1e1e24".into(),
            cursor: "#d0d070".into(),
            ansi: vec![
                "#000000".into(), // 0 black
                "#cd3131".into(), // 1 red
                "#0dbc79".into(), // 2 green
                "#e5e510".into(), // 3 yellow
                "#2472c8".into(), // 4 blue
                "#bc3fbc".into(), // 5 magenta
                "#11a8cd".into(), // 6 cyan
                "#e5e5e5".into(), // 7 white
                "#666666".into(), // 8 bright black
                "#f14c4c".into(), // 9 bright red
                "#23d18b".into(), // 10 bright green
                "#f5f543".into(), // 11 bright yellow
                "#3b8eea".into(), // 12 bright blue
                "#d670d6".into(), // 13 bright magenta
                "#29b8db".into(), // 14 bright cyan
                "#ffffff".into(), // 15 bright white
            ],
        }
    }

    /// A light terminal palette (Solarized-ish light), for the Light UI theme.
    pub fn light() -> Self {
        Self {
            name: "Light".into(),
            fg: "#2b2b2b".into(),
            bg: "#fdfdfb".into(),
            cursor: "#b07b00".into(),
            ansi: vec![
                "#2e2e2e".into(),
                "#c41a16".into(),
                "#108a2a".into(),
                "#a87800".into(),
                "#1d5fd6".into(),
                "#a435b0".into(),
                "#0e8fa6".into(),
                "#dcdcdc".into(),
                "#7a7a7a".into(),
                "#e0392f".into(),
                "#23a043".into(),
                "#c79100".into(),
                "#3b7fe0".into(),
                "#c155cf".into(),
                "#1aa9c4".into(),
                "#ffffff".into(),
            ],
        }
    }

    /// Resolve to `Color32`s, tolerating bad hex (falls back to dark defaults).
    pub fn resolve(&self) -> ResolvedTerminal {
        let d = |hex: &str, fallback: egui::Color32| parse_hex(hex).unwrap_or(fallback);
        let mut ansi = [egui::Color32::GRAY; 16];
        let fallback = TERM_FALLBACK_ANSI;
        for (i, slot) in ansi.iter_mut().enumerate() {
            *slot = self
                .ansi
                .get(i)
                .and_then(|h| parse_hex(h))
                .unwrap_or(fallback[i]);
        }
        ResolvedTerminal {
            fg: d(&self.fg, egui::Color32::from_rgb(0xcc, 0xcc, 0xcc)),
            bg: d(&self.bg, egui::Color32::from_rgb(0x1e, 0x1e, 0x24)),
            cursor: d(&self.cursor, egui::Color32::from_rgb(0xd0, 0xd0, 0x70)),
            ansi,
        }
    }
}

/// Fallback ANSI colors if a theme is missing entries.
const TERM_FALLBACK_ANSI: [egui::Color32; 16] = [
    egui::Color32::from_rgb(0, 0, 0),
    egui::Color32::from_rgb(205, 49, 49),
    egui::Color32::from_rgb(13, 188, 121),
    egui::Color32::from_rgb(229, 229, 16),
    egui::Color32::from_rgb(36, 114, 200),
    egui::Color32::from_rgb(188, 63, 188),
    egui::Color32::from_rgb(17, 168, 205),
    egui::Color32::from_rgb(229, 229, 229),
    egui::Color32::from_rgb(102, 102, 102),
    egui::Color32::from_rgb(241, 76, 76),
    egui::Color32::from_rgb(35, 209, 139),
    egui::Color32::from_rgb(245, 245, 67),
    egui::Color32::from_rgb(59, 142, 234),
    egui::Color32::from_rgb(214, 112, 214),
    egui::Color32::from_rgb(41, 184, 219),
    egui::Color32::from_rgb(255, 255, 255),
];

/// The egui data-store id under which the resolved terminal palette is stashed
/// each frame (set by the app, read by `terminal.rs`).
pub fn terminal_colors_id() -> egui::Id {
    egui::Id::new("puppy-terminal-colors")
}

/// Load the active terminal theme (`terminal.json`), defaulting to dark.
pub fn load_terminal() -> TerminalTheme {
    let Some(path) = config_path("terminal.json") else {
        return TerminalTheme::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => TerminalTheme::default(),
    }
}

/// Persist the active terminal theme to `terminal.json` (best-effort).
pub fn save_terminal(t: &TerminalTheme) {
    let Some(path) = config_path("terminal.json") else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(t) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_resolves_all_16_ansi() {
        let r = TerminalTheme::dark().resolve();
        assert_eq!(r.ansi[1], egui::Color32::from_rgb(205, 49, 49)); // red
        assert_eq!(r.ansi[15], egui::Color32::from_rgb(255, 255, 255)); // bright white
        assert_eq!(r.bg, egui::Color32::from_rgb(0x1e, 0x1e, 0x24));
    }

    #[test]
    fn missing_ansi_entries_fall_back() {
        let mut t = TerminalTheme::dark();
        t.ansi = vec!["#010203".into()]; // only one provided
        let r = t.resolve();
        assert_eq!(r.ansi[0], egui::Color32::from_rgb(1, 2, 3));
        assert_eq!(r.ansi[1], TERM_FALLBACK_ANSI[1]); // filled from fallback
    }
}
