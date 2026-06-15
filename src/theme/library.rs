//! Theme-library helpers: the ANSI color labels and the saved-theme
//! upsert/unique-name logic. Frontend-agnostic (the egui theme-editor that
//! once lived here was removed in G5; the GPUI editor is `gpui_ui::theme_ui`).

use super::ThemePalette;

/// Human labels for the 16 base ANSI colors (theme-editor swatch captions).
pub const ANSI_NAMES: [&str; 16] = [
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

/// Insert `theme` into `library`, replacing any existing entry with the same
/// name (so re-saving an edited theme updates in place).
pub fn upsert(library: &mut Vec<ThemePalette>, theme: ThemePalette) {
    if let Some(slot) = library.iter_mut().find(|t| t.name == theme.name) {
        *slot = theme;
    } else {
        library.push(theme);
    }
}

/// A name not already taken in the library (`base`, `base 2`, `base 3`, …).
pub fn unique_name(base: &str, library: &[ThemePalette]) -> String {
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
