//! Broad Unicode + emoji font coverage.
//!
//! egui's bundled fonts cover only Latin plus a small emoji subset, so other
//! scripts and many emoji render as missing-glyph boxes. We fix coverage with:
//!
//!   * a bundled **full monochrome Noto Emoji** (every emoji as an outline egui
//!     can rasterize — egui has no color-emoji support, so these are silhouettes),
//!   * **Segoe UI** (+ Segoe UI Symbol) for Latin/Cyrillic/Greek/symbols, and
//!   * Windows **CJK** fonts (YaHei, Yu Gothic, Malgun Gothic) for 中文/日本語/한국어.
//!
//! Each is added as a fallback so glyphs resolve instead of showing as tofu.

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Full monochrome emoji coverage, embedded in the binary.
const NOTO_EMOJI: &[u8] = include_bytes!("../assets/NotoEmoji-Regular.ttf");

/// Load a system font file into the definitions; returns whether it was found.
fn load(fonts: &mut FontDefinitions, key: &str, path: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => {
            fonts
                .font_data
                .insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
            true
        }
        Err(_) => false,
    }
}

/// Best-effort system font paths for symbols + CJK, per OS. Missing files are
/// skipped, so any subset present still improves coverage. (`cfg!` keeps every
/// branch compiling on every target.)
fn system_font_candidates() -> Vec<&'static str> {
    if cfg!(target_os = "windows") {
        vec![
            r"C:\Windows\Fonts\seguisym.ttf", // symbols
            r"C:\Windows\Fonts\msyh.ttc",     // Simplified Chinese
            r"C:\Windows\Fonts\YuGothR.ttc",  // Japanese
            r"C:\Windows\Fonts\malgun.ttf",   // Korean
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/PingFang.ttc", // CJK (SC/TC/JP)
            "/System/Library/Fonts/Hiragino Sans GB.ttc", // Chinese/Japanese
            "/Library/Fonts/Arial Unicode.ttf",   // broad coverage if present
        ]
    } else {
        // Linux/BSD: common Noto / DejaVu install locations across distros.
        vec![
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ]
    }
}

/// A nicer primary proportional font to prefer over egui's default, if present.
fn primary_font() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some(r"C:\Windows\Fonts\segoeui.ttf")
    } else if cfg!(target_os = "macos") {
        Some("/System/Library/Fonts/SFNS.ttf")
    } else {
        None // egui's bundled Latin font is fine on Linux
    }
}

pub fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Bundled full emoji (monochrome).
    fonts.font_data.insert(
        "noto-emoji".to_owned(),
        Arc::new(FontData::from_static(NOTO_EMOJI)),
    );

    // Load whatever system symbol/CJK fonts exist on this OS as fallbacks.
    let mut fallback_keys: Vec<String> = Vec::new();
    for (i, path) in system_font_candidates().into_iter().enumerate() {
        let key = format!("sys-{i}");
        if load(&mut fonts, &key, path) {
            fallback_keys.push(key);
        }
    }

    // Optional nicer primary Latin font for this OS.
    let primary = primary_font().is_some_and(|p| load(&mut fonts, "primary", p));

    // Fallback order: primary text first, then emoji, then system CJK/symbols.
    let add_fallbacks = |fam: &mut Vec<String>| {
        fam.push("noto-emoji".to_owned());
        for key in &fallback_keys {
            fam.push(key.clone());
        }
    };

    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        if primary {
            prop.insert(0, "primary".to_owned());
        }
        add_fallbacks(prop);
    }
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        add_fallbacks(mono);
    }

    ctx.set_fonts(fonts);
}
