//! Design tokens for the GPUI frontend, resolved once at startup.
//!
//! One source of truth: every value, including `bg`/`dim` (palette fields
//! `app_bg`/`dim_text` since Phase E), is *parsed from* the shared
//! [`ThemePalette`](crate::theme::ThemePalette) (the same hex strings the
//! egui branch renders), so the two redesign branches can't drift apart.
//! Theme switching re-resolves `RootView.tokens = Tokens::from_palette(..)`;
//! everything downstream gets the new copy on the next render (snapshot
//! pattern), and long-lived `ChatInput` entities are re-themed via
//! `set_tokens`.

use gpui::Rgba;

use crate::theme::{ThemePalette, parse_hex};

// A few tokens have no consumer yet — they are the vocabulary for Task 2.3+.
#[allow(dead_code)] // consumed by the upcoming GPUI chat/den tasks
#[derive(Clone, Copy)]
pub struct Tokens {
    /// Whether the active palette is dark-based (`ThemePalette.dark_mode`)
    /// — picks the syntect highlight theme, among other light/dark forks.
    pub dark: bool,
    /// App backdrop behind all panels (`ThemePalette.app_bg`).
    pub bg: Rgba,
    /// Dimmest text, below `weak` (`ThemePalette.dim_text`).
    pub dim: Rgba,
    pub panel: Rgba,
    /// Card surface (`ThemePalette.window` doubles as the card fill).
    pub card: Rgba,
    /// Soft hairlines/separators (`ThemePalette.stroke`).
    pub line_soft: Rgba,
    pub text: Rgba,
    pub weak: Rgba,
    pub strong: Rgba,
    /// Input wells / recessed surfaces.
    pub well: Rgba,
    pub accent: Rgba,
    pub accent_2: Rgba,
    pub accent_ink: Rgba,
    // Status colors (the design's five states).
    pub run: Rgba,
    pub think: Rgba,
    pub wait: Rgba,
    pub paused: Rgba,
    pub error: Rgba,
}

/// The active tokens, readable by entities created after a theme switch
/// (`ChatInput::new` can't reach the root). Written ONLY by the root's
/// theme apply; everything render-side still gets tokens passed down.
static CURRENT: std::sync::Mutex<Option<Tokens>> = std::sync::Mutex::new(None);

impl Tokens {
    /// Resolve from the shared amber dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&ThemePalette::dark())
    }

    /// The active theme's tokens (dark until the root applies one).
    pub fn current() -> Self {
        CURRENT
            .lock()
            .ok()
            .and_then(|g| *g)
            .unwrap_or_else(Self::dark)
    }

    /// Publish the active tokens (root-only, on theme switch/edit).
    pub fn set_current(t: Tokens) {
        if let Ok(mut g) = CURRENT.lock() {
            *g = Some(t);
        }
    }

    pub fn from_palette(p: &ThemePalette) -> Self {
        Tokens {
            dark: p.dark_mode,
            bg: hex(&p.app_bg),
            dim: hex(&p.dim_text),
            panel: hex(&p.panel),
            card: hex(&p.window),
            line_soft: hex(&p.stroke),
            text: hex(&p.text),
            weak: hex(&p.weak_text),
            strong: hex(&p.strong_text),
            well: hex(&p.extreme_bg),
            accent: hex(&p.accent),
            accent_2: hex(&p.accent2),
            accent_ink: hex(&p.accent_ink),
            run: hex(&p.status_run),
            think: hex(&p.status_think),
            wait: hex(&p.status_wait),
            paused: hex(&p.status_paused),
            error: hex(&p.status_error),
        }
    }
}

/// Palette hex (`#rrggbb`) → gpui color, through the SAME lenient parser the
/// egui side uses (mid-gray fallback; a theme typo must never crash the app).
/// Also used for the Den's relay-assigned owner colors.
pub(crate) fn hex(s: &str) -> Rgba {
    let (r, g, b) = parse_hex(s).unwrap_or((0xa0, 0xa0, 0xa0));
    gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_track_the_shared_palette() {
        let t = Tokens::dark();
        // Amber brand + card surface come from ThemePalette::dark(), not
        // from constants duplicated here.
        assert_eq!(t.accent, gpui::rgb(0xe7ab4d));
        assert_eq!(t.card, gpui::rgb(0x1e1e26));
        assert_eq!(t.panel, gpui::rgb(0x1a1a20));
        // bg/dim come from the palette's app_bg/dim_text defaults now.
        assert_eq!(t.bg, gpui::rgb(0x121217));
        assert_eq!(t.dim, gpui::rgb(0x696977));
        // The light preset resolves its own backdrop (no dark constant).
        let l = Tokens::from_palette(&ThemePalette::light());
        assert_eq!(l.bg, gpui::rgb(0xe9e9ee));
        assert!(t.dark && !l.dark);
    }

    #[test]
    fn bad_hex_falls_back_to_gray() {
        assert_eq!(hex("not-a-color"), gpui::rgb(0xa0a0a0));
    }
}
