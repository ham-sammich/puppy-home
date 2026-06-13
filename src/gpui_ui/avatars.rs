//! Avatar picker (QW8): emoji avatars for YOU (transcript "you" rows,
//! default \u{1f9d1}) and YOUR PUPPY (transcript/ask/empty-state/title
//! chip/dashboard lede, default \u{1f436}).
//!
//! One floating panel (toolbar identity chip toggles it): a target
//! switch (You / Puppy), a curated grid, and a free-text input that
//! accepts ANY emoji. Picks apply + persist immediately (session.json
//! `user_avatar` / `puppy_avatar` — shared serde fields, sync queued
//! for redesign/egui).
//!
//! Den note: roster/feed avatars for OTHER members stay the default —
//! the relay protocol has no avatar slot (ledgered; not extending the
//! wire for this).

use gpui::prelude::*;
use gpui::{AnyElement, Entity, FontWeight, IntoElement, div, px};

use crate::gpui_ui::widgets;
use crate::gpui_ui::{DashAction, RootView, Tokens};

pub const USER_DEFAULT: &str = "\u{1f9d1}";
pub const PUPPY_DEFAULT: &str = "\u{1f436}";

/// An avatar value is EITHER a single emoji OR an absolute path to a chosen
/// photo (F11). Emojis have no path separators; a photo always does.
pub fn is_photo(value: &str) -> bool {
    value.contains('/') || value.contains('\\')
}

/// For INLINE TEXT contexts ("{emoji} asks"): a photo path can't be inlined,
/// so fall back to the kind's default emoji.
pub fn inline(value: &str, default: &str) -> String {
    if value.is_empty() || is_photo(value) {
        default.to_string()
    } else {
        value.to_string()
    }
}

/// Avatar glyph that FILLS its (already-sized) parent box: the chosen photo
/// (cover-fit, clipped to `radius`) or the emoji at `font_px` (F11).
pub fn fill_parent(value: &str, font_px: f32, radius: f32) -> AnyElement {
    if is_photo(value) {
        gpui::img(std::path::PathBuf::from(value))
            .size_full()
            .rounded(px(radius))
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .text_size(px(font_px))
            .child(value.to_string())
            .into_any_element()
    }
}

/// Self-sized avatar glyph (for contexts without a sized container): a
/// `diameter`-px photo clipped to `radius`, or the centered emoji (F11).
pub fn boxed(value: &str, diameter: f32, radius: f32) -> AnyElement {
    if is_photo(value) {
        gpui::img(std::path::PathBuf::from(value))
            .size(px(diameter))
            .rounded(px(radius))
            .object_fit(gpui::ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .size(px(diameter))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(diameter * 0.66))
            .child(value.to_string())
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarKind {
    User,
    Puppy,
}

#[derive(Clone, Debug)]
pub enum AvatarAction {
    Toggle,
    Target(AvatarKind),
    /// Apply this emoji to the current target (empty = reset to default).
    Pick(String),
    /// Apply whatever's in the custom input to the current target.
    ApplyCustom,
    /// Open the native file picker and set the current target to a photo (F11).
    PickPhoto,
}

/// Panel state (the chosen avatars themselves live on RootView and in
/// session.json).
pub struct AvatarUi {
    pub open: bool,
    pub target: AvatarKind,
}

impl Default for AvatarUi {
    fn default() -> Self {
        Self {
            open: false,
            target: AvatarKind::Puppy,
        }
    }
}

/// The curated grid: pack animals first, then people, then the weird
/// fun ones. Any emoji works via the custom input.
const CHOICES: &[&str] = &[
    "\u{1f436}",
    "\u{1f415}",
    "\u{1f429}",
    "\u{1f43a}",
    "\u{1f98a}",
    "\u{1f431}",
    "\u{1f981}",
    "\u{1f42f}",
    "\u{1f43c}",
    "\u{1f428}",
    "\u{1f43b}",
    "\u{1f439}",
    "\u{1f430}",
    "\u{1f99d}",
    "\u{1f984}",
    "\u{1f409}",
    "\u{1f996}",
    "\u{1f419}",
    "\u{1f985}",
    "\u{1f989}",
    "\u{1f427}",
    "\u{1f422}",
    "\u{1f41d}",
    "\u{1f980}",
    "\u{1f9d1}",
    "\u{1f468}",
    "\u{1f469}",
    "\u{1f9d4}",
    "\u{1f471}",
    "\u{1f9d9}",
    "\u{1f977}",
    "\u{1f9b8}",
    "\u{1f916}",
    "\u{1f47d}",
    "\u{1f4bb}",
    "\u{1f525}",
    "\u{2b50}",
    "\u{1f680}",
    "\u{1f9e0}",
    "\u{1f3af}",
];

/// Copy a chosen image into `<data-dir>/avatars/` and return its stored
/// absolute path (F11). `None` if it isn't an image or the copy fails. The
/// filename carries a timestamp so gpui's image cache never serves a stale
/// frame after a re-pick.
pub fn store_photo(src: &std::path::Path, kind: AvatarKind) -> Option<String> {
    let ext = src.extension()?.to_str()?.to_lowercase();
    // Keep in sync with the rfd picker filter; only formats the `image`
    // crate (gpui's decoder) can actually render.
    if !matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "jfif" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "ico"
    ) {
        return None;
    }
    let dir = crate::session::data_dir()?.join("avatars");
    std::fs::create_dir_all(&dir).ok()?;
    let who = match kind {
        AvatarKind::User => "user",
        AvatarKind::Puppy => "puppy",
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dest = dir.join(format!("{who}-{stamp}.{ext}"));
    std::fs::copy(src, &dest).ok()?;
    Some(dest.to_string_lossy().into_owned())
}

fn act(
    root: &Entity<RootView>,
    a: AvatarAction,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    let root = root.clone();
    move |_, _, cx| {
        let a = a.clone();
        root.update(cx, |r, cx| r.dispatch(DashAction::Avatar(a), cx));
    }
}

/// The floating picker panel (anchored under the toolbar identity chip).
pub fn panel(
    t: &Tokens,
    ui: &AvatarUi,
    user_avatar: &str,
    puppy_avatar: &str,
    custom_input: Option<&Entity<crate::gpui_ui::input::ChatInput>>,
    root: &Entity<RootView>,
) -> impl IntoElement {
    let target_chip = |label: String, kind: AvatarKind, current: &str| {
        let on = ui.target == kind;
        div()
            .id(match kind {
                AvatarKind::User => "avatar-tgt-user",
                AvatarKind::Puppy => "avatar-tgt-puppy",
            })
            .px_2()
            .py_1()
            .rounded(px(8.))
            .cursor_pointer()
            .border_1()
            .when(on, |d| {
                d.bg(widgets::alpha(t.accent, 0.15)).border_color(t.accent)
            })
            .when(!on, |d| d.border_color(t.line_soft))
            .flex()
            .items_center()
            .gap_1p5()
            .text_size(px(12.))
            .text_color(if on { t.accent } else { t.weak })
            .child(boxed(current, 18., 5.))
            .child(label)
            .on_click(act(root, AvatarAction::Target(kind)))
    };

    let grid = div().flex().flex_wrap().gap_1().children(
        CHOICES
            .iter()
            .enumerate()
            .map(|(i, e)| {
                div()
                    .id(("avatar-pick", i))
                    .w(px(34.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|d| d.bg(widgets::alpha(t.accent, 0.15)))
                    .text_size(px(16.))
                    .child(e.to_string())
                    .on_click(act(root, AvatarAction::Pick(e.to_string())))
            })
            .collect::<Vec<_>>(),
    );

    div()
        .absolute()
        .top(px(44.))
        .left(px(12.))
        .w(px(330.))
        .p_3()
        .rounded(px(10.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap_2()
        .occlude()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(13.5))
                        .text_color(t.text)
                        .child("Profile pictures"),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(t.dim)
                        .child("Upload a photo or pick an emoji \u{2014} for you and your puppy"),
                ),
        )
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(target_chip("you".into(), AvatarKind::User, user_avatar))
                .child(target_chip("puppy".into(), AvatarKind::Puppy, puppy_avatar)),
        )
        // Photo upload is the headline action — primary button, up top.
        .child(
            widgets::primary_btn(t, "\u{1f5bc} Upload a photo\u{2026}")
                .id("avatar-photo")
                .on_click(act(root, AvatarAction::PickPhoto)),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(t.dim)
                .child("or pick an emoji:"),
        )
        .child(grid)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        // min_w_0 + overflow_hidden so the text input shrinks
                        // to the row instead of reporting its full content
                        // width and overflowing the panel.
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .px_2()
                        .py_1()
                        .rounded(px(8.))
                        .bg(t.well)
                        .border_1()
                        .border_color(t.line_soft)
                        .text_size(px(13.))
                        .children(custom_input.cloned()),
                )
                .child(
                    widgets::btn(t, "Use")
                        .id("avatar-custom")
                        .on_click(act(root, AvatarAction::ApplyCustom)),
                ),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    widgets::btn(t, "Reset to default")
                        .id("avatar-reset")
                        .on_click(act(root, AvatarAction::Pick(String::new()))),
                )
                .child(
                    widgets::btn(t, "Close")
                        .id("avatar-close")
                        .on_click(act(root, AvatarAction::Toggle)),
                ),
        )
}
