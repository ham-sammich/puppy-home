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

pub fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Bundled full emoji (monochrome).
    fonts
        .font_data
        .insert("noto-emoji".to_owned(), Arc::new(FontData::from_static(NOTO_EMOJI)));

    // Windows system fonts (best-effort; `.ttc` collections load face 0).
    let segoe = load(&mut fonts, "segoe-ui", r"C:\Windows\Fonts\segoeui.ttf");
    let symbol = load(&mut fonts, "segoe-symbol", r"C:\Windows\Fonts\seguisym.ttf");
    let zh = load(&mut fonts, "cjk-sc", r"C:\Windows\Fonts\msyh.ttc"); // Simplified Chinese
    let ja = load(&mut fonts, "cjk-ja", r"C:\Windows\Fonts\YuGothR.ttc"); // Japanese
    let ko = load(&mut fonts, "cjk-ko", r"C:\Windows\Fonts\malgun.ttf"); // Korean

    // Fallback order: primary text first, then emoji, symbols, and CJK so any
    // glyph the earlier fonts lack still resolves.
    let fallbacks = |fam: &mut Vec<String>| {
        fam.push("noto-emoji".to_owned());
        if symbol {
            fam.push("segoe-symbol".to_owned());
        }
        if ja {
            fam.push("cjk-ja".to_owned());
        }
        if zh {
            fam.push("cjk-sc".to_owned());
        }
        if ko {
            fam.push("cjk-ko".to_owned());
        }
    };

    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        if segoe {
            prop.insert(0, "segoe-ui".to_owned());
        }
        fallbacks(prop);
    }
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        fallbacks(mono);
    }

    ctx.set_fonts(fonts);
}
