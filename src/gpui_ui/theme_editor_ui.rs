//! The theme-editor overlay render (egui `theme/editor.rs` window at
//! parity): saved-theme library, name/dark-base/start-from rows, the
//! palette color rows (live swatch + hex input), and the terminal palette
//! (fg/bg/cursor + 16 ANSI). Dispatch + state live in `theme_ui`.

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers_ui::{disabled_btn, small};
use crate::gpui_ui::theme_ui::{
    T_ANSI, T_COLORS, T_NAME, T_TERM, ThemeAction, palette_slots, tact,
};
use crate::gpui_ui::tokens::hex;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{RootView, Tokens};
use crate::session::Theme;
use crate::theme::{ANSI_NAMES, TerminalTheme, ThemePalette};

/// One color row: live swatch + hex input + label (egui `color_row`; the
/// native picker button has no counterpart at this pin — hex is canonical).
fn color_row(
    t: &Tokens,
    label: &'static str,
    value: &str,
    input: Option<&Entity<ChatInput>>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .rounded(px(4.))
                .border_1()
                .border_color(t.line_soft)
                .bg(hex(value)),
        )
        .children(input.map(|i| {
            div()
                .w(px(86.))
                .flex_none()
                .px_1p5()
                .py_0p5()
                .rounded(px(6.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .child(i.clone())
        }))
        .child(small(t, label, t.weak))
        .into_any_element()
}

/// The theme-editor overlay (egui's "Theme editor" window).
pub(crate) fn editor_overlay(
    t: Tokens,
    root: &Entity<RootView>,
    inputs: &[Entity<ChatInput>],
    palette: &ThemePalette,
    term: &TerminalTheme,
    library_names: &[String],
    theme: &Theme,
) -> AnyElement {
    let named = !palette.name.trim().is_empty();
    let exists = library_names.contains(&palette.name);

    // Saved-theme library: clickable chips (egui uses a combo box — same
    // load-by-name behavior, flat presentation).
    let mut saved = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .child(small(&t, "Saved:", t.weak));
    if library_names.is_empty() {
        saved = saved.child(small(&t, "(none yet)", t.dim));
    } else {
        for (i, name) in library_names.iter().enumerate() {
            let sel = *name == palette.name;
            saved = saved.child(
                div()
                    .id(("theme-saved", i as u64))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(if sel { alpha(t.accent, 0.16) } else { t.well })
                    .border_1()
                    .border_color(if sel {
                        alpha(t.accent, 0.7)
                    } else {
                        t.line_soft
                    })
                    .text_size(px(11.))
                    .text_color(if sel { t.accent } else { t.weak })
                    .cursor_pointer()
                    .hover(|d| d.border_color(alpha(t.accent, 0.5)))
                    .child(name.clone())
                    .on_click(tact(root, ThemeAction::LoadSaved(name.clone()))),
            );
        }
    }
    saved = saved.child(
        widgets::btn(&t, "New")
            .id("theme-new")
            .on_click(tact(root, ThemeAction::EditorNew)),
    );

    // Name + dark-base + start-from row.
    let name_row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(small(&t, "Name:", t.weak))
        .children(inputs.get(T_NAME).map(|i| {
            div()
                .w(px(150.))
                .px_1p5()
                .py_0p5()
                .rounded(px(6.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .text_size(px(11.5))
                .child(i.clone())
        }))
        .child(
            widgets::btn(
                &t,
                if palette.dark_mode {
                    "dark base: on"
                } else {
                    "dark base: off"
                },
            )
            .id("theme-darkbase")
            .when(palette.dark_mode, |d| d.border_color(alpha(t.accent, 0.6)))
            .on_click(tact(root, ThemeAction::ToggleDarkBase)),
        );
    let start_row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(small(&t, "Start from:", t.weak))
        .child(
            widgets::btn(&t, "Dark")
                .id("theme-start-dark")
                .on_click(tact(root, ThemeAction::StartFrom(true))),
        )
        .child(
            widgets::btn(&t, "Light")
                .id("theme-start-light")
                .on_click(tact(root, ThemeAction::StartFrom(false))),
        );

    // The 25 palette rows (values from the working buffer = live).
    let mut p = palette.clone();
    let rows: Vec<AnyElement> = palette_slots(&mut p)
        .into_iter()
        .enumerate()
        .map(|(ix, (label, value))| color_row(&t, label, value, inputs.get(T_COLORS + ix)))
        .collect();

    let save_row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(if named {
            widgets::primary_btn(&t, "Save theme")
                .id("theme-save")
                .on_click(tact(root, ThemeAction::SaveTheme))
                .into_any_element()
        } else {
            disabled_btn(&t, "Save theme").into_any_element()
        })
        .child(if exists {
            widgets::btn(&t, "Delete")
                .id("theme-delete")
                .on_click(tact(root, ThemeAction::DeleteTheme))
                .into_any_element()
        } else {
            disabled_btn(&t, "Delete").into_any_element()
        });

    // Terminal palette section.
    let term_rows: Vec<AnyElement> = [
        ("Foreground", &term.fg),
        ("Background", &term.bg),
        ("Cursor", &term.cursor),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (label, value))| color_row(&t, label, value, inputs.get(T_TERM + i)))
    .chain(ANSI_NAMES.iter().enumerate().map(|(i, name)| {
        let value = term.ansi.get(i).map(String::as_str).unwrap_or("#888888");
        color_row(&t, name, value, inputs.get(T_ANSI + i))
    }))
    .collect();

    let body = div()
        .id("theme-editor-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(saved)
        .child(name_row)
        .child(start_row)
        .children(rows)
        .child(save_row)
        .children(
            crate::theme::themes_path()
                .map(|p| small(&t, p.display().to_string(), t.dim).into_any_element()),
        )
        .child(div().h(px(1.)).bg(t.line_soft))
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child("Terminal theme"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(small(&t, "Start from:", t.weak))
                .child(
                    widgets::btn(&t, "Dark")
                        .id("term-start-dark")
                        .on_click(tact(root, ThemeAction::TermStartFrom(true))),
                )
                .child(
                    widgets::btn(&t, "Light")
                        .id("term-start-light")
                        .on_click(tact(root, ThemeAction::TermStartFrom(false))),
                ),
        )
        .children(term_rows)
        .child(
            div().child(
                widgets::btn(&t, "Save terminal")
                    .id("term-save")
                    .tooltip(widgets::text_tip("Write terminal.json".into()))
                    .on_click(tact(root, ThemeAction::TermSave)),
            ),
        );

    let panel = div()
        .occlude()
        .w(px(430.))
        .max_w_full()
        .h(px(620.))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(13.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child("Theme editor"),
                )
                .child(div().flex_1())
                .child(small(&t, format!("active: {}", theme.label()), t.dim))
                .child(div().w(px(8.)))
                .child(
                    widgets::btn(&t, "Close")
                        .id("theme-editor-close")
                        .on_click(tact(root, ThemeAction::EditorClose)),
                ),
        )
        .child(body);

    gpui::deferred(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(alpha(t.bg, 0.6))
            .child(panel),
    )
    .with_priority(220)
    .into_any_element()
}
