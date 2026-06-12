//! GPUI asset source: the bundled fonts from `assets/`, embedded at compile
//! time (same binaries the egui branch bundles — Space Grotesk for UI type,
//! JetBrains Mono for numbers/paths, Noto Emoji for glyph fallback).

use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};

/// `(virtual path, bytes)` for every embedded asset.
const FONTS: &[(&str, &[u8])] = &[
    (
        "fonts/SpaceGrotesk-Regular.ttf",
        include_bytes!("../../assets/SpaceGrotesk-Regular.ttf"),
    ),
    (
        "fonts/SpaceGrotesk-Bold.ttf",
        include_bytes!("../../assets/SpaceGrotesk-Bold.ttf"),
    ),
    (
        "fonts/JetBrainsMono-Regular.ttf",
        include_bytes!("../../assets/JetBrainsMono-Regular.ttf"),
    ),
    (
        "fonts/JetBrainsMono-Bold.ttf",
        include_bytes!("../../assets/JetBrainsMono-Bold.ttf"),
    ),
    (
        "fonts/NotoEmoji-Regular.ttf",
        include_bytes!("../../assets/NotoEmoji-Regular.ttf"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FONTS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FONTS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Register every bundled font with the text system, through the asset
/// source (so the font bytes have exactly one home).
pub fn register_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = cx
        .asset_source()
        .list("fonts/")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| cx.asset_source().load(&path).ok().flatten())
        .collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("puppy-home: failed to register bundled fonts: {e}");
    }
}
