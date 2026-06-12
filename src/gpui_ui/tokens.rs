//! Design tokens for the GPUI frontend, resolved once at startup.
//!
//! One source of truth: every value that exists in the shared
//! [`ThemePalette`](crate::theme::ThemePalette) amber preset is *parsed from
//! it* (the same hex strings the egui branch renders), so the two redesign
//! branches can't drift apart. The only token the palette doesn't model is
//! `bg` — the app backdrop *behind* the panels (egui uses `panel` as its
//! outermost fill) — which keeps the GPUI_GUIDE's `#121217`.

use gpui::Rgba;

use crate::theme::{ThemePalette, parse_hex};
use crate::workspace::InstanceStatus;

// Several tokens (panel/strong/well/accent_2/accent_ink) have no consumer in
// the Task 2.1 scaffold yet — they are the vocabulary for Task 2.2+.
#[allow(dead_code)] // consumed by the upcoming GPUI dashboard tasks
pub struct Tokens {
    /// App backdrop behind all panels (GPUI_GUIDE §2; no palette equivalent).
    pub bg: Rgba,
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

impl Tokens {
    /// Resolve from the shared amber dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&ThemePalette::dark())
    }

    pub fn from_palette(p: &ThemePalette) -> Self {
        Tokens {
            bg: gpui::rgb(0x121217),
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

    /// The dot/label color for an instance status, in token terms.
    pub fn status_color(&self, status: InstanceStatus) -> Rgba {
        match status {
            InstanceStatus::Starting | InstanceStatus::Idle => self.weak,
            InstanceStatus::Running => self.run,
            InstanceStatus::Thinking | InstanceStatus::ToolCalling => self.think,
            InstanceStatus::WaitingForInput => self.wait,
            InstanceStatus::Paused => self.paused,
            InstanceStatus::Dead => self.error,
        }
    }
}

/// Palette hex (`#rrggbb`) → gpui color, through the SAME lenient parser the
/// egui side uses (mid-gray fallback; a theme typo must never crash the app).
fn hex(s: &str) -> Rgba {
    let c = parse_hex(s).unwrap_or(eframe::egui::Color32::GRAY);
    gpui::rgb(((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32)
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
        // The one deliberate non-palette token.
        assert_eq!(t.bg, gpui::rgb(0x121217));
    }

    #[test]
    fn bad_hex_falls_back_to_gray() {
        assert_eq!(hex("not-a-color"), gpui::rgb(0xa0a0a0));
    }
}
