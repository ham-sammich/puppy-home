//! The embedded terminal's color theme: foreground/background/cursor + the 16
//! base ANSI colors. Persisted to `terminal.json` and handed to `terminal.rs`
//! each frame and handed to the GPUI terminal renderer, which parses the hex
//! strings into its own color type.

use serde::{Deserialize, Serialize};

use super::config_path;

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

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::dark()
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
    fn dark_has_all_16_ansi() {
        let t = TerminalTheme::dark();
        assert_eq!(t.ansi.len(), 16);
        assert_eq!(t.ansi[1], "#cd3131"); // red
        assert_eq!(t.ansi[15], "#ffffff"); // bright white
        assert_eq!(t.bg, "#1e1e24");
    }

    #[test]
    fn roundtrips_via_serde() {
        let t = TerminalTheme::light();
        let j = serde_json::to_string(&t).unwrap();
        let back: TerminalTheme = serde_json::from_str(&j).unwrap();
        assert_eq!(t, back);
    }
}
