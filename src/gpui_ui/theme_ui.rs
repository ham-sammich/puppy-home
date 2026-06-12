//! Theme switching + the theme editor — egui's top-bar `Theme:` menu and
//! `theme/editor.rs` window at parity, dressed in the GPUI tokens.
//!
//! Switching re-resolves `RootView.tokens = Tokens::from_palette(..)`
//! (published via `Tokens::set_current` for entities created later) and
//! pushes the new tokens into every live `ChatInput`; everything else
//! follows on the next render via the snapshot pattern.
//!
//! The editor matches egui's: a saved-theme library (load / New / Save /
//! Delete over themes.json), Start-from presets, per-field color rows with
//! live preview (edits implicitly select a Custom theme, exactly like
//! egui's `changed` outcome), and the terminal palette (fg/bg/cursor + 16
//! ANSI slots, written to terminal.json). Color rows are hex fields with a
//! live swatch — egui's rows are hex fields too; only its native
//! color-picker *button* has no GPUI counterpart at this pin (deviation).

use gpui::{AnyElement, Entity, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers_ui::small;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::session::Theme;
use crate::theme::{
    TerminalTheme, ThemePalette, palette_for, save_terminal, save_themes, unique_name, upsert,
};

/// Theme interactions, nested under `DashAction::Theme`.
#[derive(Clone, Debug)]
pub enum ThemeAction {
    PickerToggle,
    PickerClose,
    Pick(Theme),
    EditorOpen,
    EditorClose,
    /// New library entry (dark-based, unique name).
    EditorNew,
    /// Reset the working palette to a preset, keeping the name (true=dark).
    StartFrom(bool),
    ToggleDarkBase,
    /// Load a saved theme into the working buffer (and select it).
    LoadSaved(String),
    SaveTheme,
    DeleteTheme,
    /// Reset the terminal buffer to a preset (true=dark).
    TermStartFrom(bool),
    TermSave,
}

// --- theme-input pool indices --------------------------------------------
pub(crate) const T_NAME: usize = 0;
pub(crate) const T_COLORS: usize = 1; // ..T_COLORS+25: the palette color fields
pub(crate) const N_COLORS: usize = 25;
pub(crate) const T_TERM: usize = T_COLORS + N_COLORS; // fg, bg, cursor
pub(crate) const T_ANSI: usize = T_TERM + 3; // 16 slots
pub(crate) const T_POOL: usize = T_ANSI + 16;

/// The palette's editable color fields, in egui-editor row order. One
/// place owns the (label, field) pairing for seeding AND read-back.
pub(crate) fn palette_slots(p: &mut ThemePalette) -> Vec<(&'static str, &mut String)> {
    vec![
        ("Text", &mut p.text),
        ("Weak text", &mut p.weak_text),
        ("Strong text", &mut p.strong_text),
        ("Accent", &mut p.accent),
        ("Selection", &mut p.selection),
        ("Panel bg", &mut p.panel),
        ("Window bg", &mut p.window),
        ("Faint bg", &mut p.faint_bg),
        ("Extreme bg", &mut p.extreme_bg),
        ("Code bg", &mut p.code_bg),
        ("Widget bg", &mut p.widget_bg),
        ("Widget hover", &mut p.widget_hover),
        ("Widget active", &mut p.widget_active),
        ("Stroke", &mut p.stroke),
        ("Warn", &mut p.warn),
        ("Error", &mut p.error),
        ("Accent 2", &mut p.accent2),
        ("Accent ink", &mut p.accent_ink),
        ("Status: run", &mut p.status_run),
        ("Status: think", &mut p.status_think),
        ("Status: wait", &mut p.status_wait),
        ("Status: paused", &mut p.status_paused),
        ("Status: error", &mut p.status_error),
        ("App backdrop", &mut p.app_bg),
        ("Dim text", &mut p.dim_text),
    ]
}

impl RootView {
    pub(crate) fn dispatch_theme(&mut self, action: ThemeAction, cx: &mut gpui::Context<Self>) {
        match action {
            ThemeAction::PickerToggle => self.theme_picker_open = !self.theme_picker_open,
            ThemeAction::PickerClose => self.theme_picker_open = false,
            ThemeAction::Pick(theme) => {
                self.theme_picker_open = false;
                self.theme = theme;
                // Sync the editor buffer to a freshly-picked custom theme.
                if let Theme::Custom(name) = &self.theme
                    && let Some(p) = self.themes.iter().find(|t| &t.name == name)
                {
                    self.theme_palette = p.clone();
                    self.seed_theme_inputs(cx);
                }
                let p = palette_for(&self.theme, &self.themes);
                self.apply_palette(&p, cx);
                self.save_prefs();
            }
            ThemeAction::EditorOpen => {
                self.theme_picker_open = false;
                self.ensure_theme_inputs(cx);
                self.seed_theme_inputs(cx);
                self.theme_editor_open = true;
            }
            ThemeAction::EditorClose => {
                self.theme_editor_open = false;
                self.save_prefs(); // persist whatever the edits selected
            }
            ThemeAction::EditorNew => {
                let mut p = ThemePalette::dark();
                p.name = unique_name("My theme", &self.themes);
                self.theme_palette = p;
                self.seed_theme_inputs(cx);
                self.theme_changed(cx);
            }
            ThemeAction::StartFrom(dark) => {
                let name = self.theme_palette.name.clone();
                self.theme_palette = if dark {
                    ThemePalette::dark()
                } else {
                    ThemePalette::light()
                };
                self.theme_palette.name = name;
                self.seed_theme_inputs(cx);
                self.theme_changed(cx);
            }
            ThemeAction::ToggleDarkBase => {
                self.theme_palette.dark_mode = !self.theme_palette.dark_mode;
                self.theme_changed(cx);
            }
            ThemeAction::LoadSaved(name) => {
                if let Some(p) = self.themes.iter().find(|t| t.name == name) {
                    self.theme_palette = p.clone();
                    self.seed_theme_inputs(cx);
                    self.theme = Theme::Custom(name);
                    let p = self.theme_palette.clone();
                    self.apply_palette(&p, cx);
                    self.save_prefs();
                }
            }
            ThemeAction::SaveTheme => {
                self.theme_read_back(cx);
                if self.theme_palette.name.trim().is_empty() {
                    return; // render gates the button; belt and braces
                }
                upsert(&mut self.themes, self.theme_palette.clone());
                save_themes(&self.themes);
                self.theme = Theme::Custom(self.theme_palette.name.clone());
                let p = self.theme_palette.clone();
                self.apply_palette(&p, cx);
                self.save_prefs();
            }
            ThemeAction::DeleteTheme => {
                let name = self.theme_palette.name.clone();
                self.themes.retain(|t| t.name != name);
                save_themes(&self.themes);
            }
            ThemeAction::TermStartFrom(dark) => {
                self.terminal_theme = if dark {
                    TerminalTheme::dark()
                } else {
                    TerminalTheme::light()
                };
                self.seed_theme_inputs(cx);
                self.term_colors = super::terminal::TermColors::from_theme(&self.terminal_theme);
            }
            ThemeAction::TermSave => save_terminal(&self.terminal_theme),
        }
        cx.notify();
    }

    /// Apply a palette app-wide: re-resolve the tokens, publish them for
    /// future entities, and push them into every live input.
    pub(crate) fn apply_palette(&mut self, p: &ThemePalette, cx: &mut gpui::Context<Self>) {
        self.tokens = Tokens::from_palette(p);
        Tokens::set_current(self.tokens);
        let inputs = self.all_inputs();
        for input in inputs {
            input.update(cx, |i, cx| i.set_tokens(self.tokens, cx));
        }
    }

    /// Every live `ChatInput` entity the root owns (for re-theming).
    fn all_inputs(&self) -> Vec<Entity<ChatInput>> {
        let mut v: Vec<Entity<ChatInput>> = Vec::new();
        v.extend(self.chat_inputs.values().cloned());
        v.extend(self.editor_inputs.values().cloned());
        v.extend(self.commit_inputs.values().cloned());
        v.extend(self.mgr_inputs.iter().cloned());
        v.extend(self.remote_inputs.iter().cloned());
        v.extend(self.theme_inputs.iter().cloned());
        for opt in [
            &self.answer_input,
            &self.den_feed_input,
            &self.den_task_input,
            &self.den_join_addr,
            &self.den_join_room,
            &self.den_join_user,
            &self.sessions_filter_input,
            &self.tree_op_input,
            &self.branch_input,
            &self.creds_user_input,
            &self.creds_pass_input,
            &self.mgr_paste_input,
        ] {
            v.extend(opt.iter().cloned());
        }
        v
    }

    /// egui's `changed` outcome: live-preview the working palette and, if a
    /// preset was active, implicitly select the working custom theme.
    fn theme_changed(&mut self, cx: &mut gpui::Context<Self>) {
        if !matches!(self.theme, Theme::Custom(_)) {
            self.theme = Theme::Custom(self.theme_palette.name.clone());
        }
        let p = self.theme_palette.clone();
        self.apply_palette(&p, cx);
        self.term_colors = super::terminal::TermColors::from_theme(&self.terminal_theme);
    }

    /// Read every editor field back into the working buffers and live-apply
    /// (driven by the inputs' `Edited` events while the editor is open).
    pub(crate) fn theme_read_back(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.theme_editor_open || self.theme_inputs.len() < T_POOL {
            return;
        }
        let texts: Vec<String> = self
            .theme_inputs
            .iter()
            .map(|i| i.read(cx).text().to_string())
            .collect();
        self.theme_palette.name = texts[T_NAME].clone();
        for (ix, (_, slot)) in palette_slots(&mut self.theme_palette)
            .into_iter()
            .enumerate()
        {
            *slot = texts[T_COLORS + ix].clone();
        }
        self.terminal_theme.fg = texts[T_TERM].clone();
        self.terminal_theme.bg = texts[T_TERM + 1].clone();
        self.terminal_theme.cursor = texts[T_TERM + 2].clone();
        if self.terminal_theme.ansi.len() < 16 {
            self.terminal_theme.ansi.resize(16, "#888888".to_string());
        }
        self.terminal_theme.ansi[..16].clone_from_slice(&texts[T_ANSI..T_ANSI + 16]);
        self.theme_changed(cx);
    }

    fn ensure_theme_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        while self.theme_inputs.len() < T_POOL {
            let entity = cx.new(|cx| ChatInput::new("", cx));
            let sub = cx.subscribe(
                &entity,
                |this: &mut Self, _, ev: &crate::gpui_ui::InputEvent, cx| {
                    if matches!(ev, crate::gpui_ui::InputEvent::Edited) {
                        this.theme_read_back(cx);
                    }
                    cx.notify();
                },
            );
            self.theme_inputs.push(entity);
            self.chat_subs.push(sub);
        }
    }

    /// Seed every editor field from the working buffers.
    fn seed_theme_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        if self.theme_inputs.len() < T_POOL {
            return; // editor never opened yet
        }
        let mut p = self.theme_palette.clone();
        let mut texts = vec![p.name.clone()];
        texts.extend(palette_slots(&mut p).into_iter().map(|(_, s)| s.clone()));
        texts.push(self.terminal_theme.fg.clone());
        texts.push(self.terminal_theme.bg.clone());
        texts.push(self.terminal_theme.cursor.clone());
        for i in 0..16 {
            texts.push(
                self.terminal_theme
                    .ansi
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "#888888".to_string()),
            );
        }
        for (input, text) in self.theme_inputs.clone().into_iter().zip(texts) {
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }
}

/// Click handler funneling a theme action through the root dispatch.
pub(crate) fn tact(
    root: &Entity<RootView>,
    a: ThemeAction,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    let root = root.clone();
    move |_, _, cx| {
        let a = a.clone();
        root.update(cx, |r, cx| r.dispatch(DashAction::Theme(a), cx));
    }
}

/// The toolbar `Theme: {label}` button + its anchored picker popover
/// (egui's top-bar theme menu: Dark / Light / customs / Edit themes…).
pub(crate) fn picker(
    t: &Tokens,
    root: &Entity<RootView>,
    theme: &Theme,
    names: &[String],
    open: bool,
) -> AnyElement {
    let button = widgets::btn(t, format!("Theme: {}", theme.label()))
        .id("tb-theme")
        .on_click(tact(root, ThemeAction::PickerToggle));
    if !open {
        return div().relative().child(button).into_any_element();
    }

    let row = |id: (&'static str, u64), label: String, selected: bool, action: ThemeAction| {
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |d| d.bg(alpha(t.accent, 0.12)))
            .hover(|d| d.bg(t.well))
            .text_size(px(12.))
            .text_color(if selected { t.accent } else { t.text })
            .child(label)
            .on_click(tact(root, action))
            .into_any_element()
    };

    let mut panel = div()
        .occlude()
        .absolute()
        .top(px(30.))
        .right_0()
        .min_w(px(190.))
        .max_h(px(320.))
        .id("theme-pop-scroll")
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_1()
        .rounded(px(9.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .on_mouse_down_out({
            let root = root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Theme(ThemeAction::PickerClose), cx)
                });
            }
        })
        .child(row(
            ("theme-pick", 0),
            "Dark".into(),
            *theme == Theme::Dark,
            ThemeAction::Pick(Theme::Dark),
        ))
        .child(row(
            ("theme-pick", 1),
            "Light".into(),
            *theme == Theme::Light,
            ThemeAction::Pick(Theme::Light),
        ));
    if !names.is_empty() {
        panel = panel.child(small(t, "Custom", t.dim).px_2());
        for (i, name) in names.iter().enumerate() {
            let sel = matches!(theme, Theme::Custom(n) if n == name);
            panel = panel.child(row(
                ("theme-custom", i as u64),
                name.clone(),
                sel,
                ThemeAction::Pick(Theme::Custom(name.clone())),
            ));
        }
    }
    panel = panel.child(div().h(px(1.)).bg(t.line_soft)).child(row(
        ("theme-pick", 2),
        "Edit themes\u{2026}".into(),
        false,
        ThemeAction::EditorOpen,
    ));

    div()
        .relative()
        .child(button)
        .child(gpui::deferred(panel).with_priority(100))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ANSI_NAMES;

    #[test]
    fn palette_slots_cover_every_color_field_once() {
        let mut p = ThemePalette::dark();
        let slots = palette_slots(&mut p);
        assert_eq!(slots.len(), N_COLORS);
        // No duplicate labels (a copy-paste row would silently shadow one).
        let mut labels: Vec<&str> = slots.iter().map(|(l, _)| *l).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), N_COLORS);
    }

    #[test]
    fn input_pool_layout_is_consistent() {
        assert_eq!(T_NAME, 0);
        assert_eq!(T_COLORS, 1);
        assert_eq!(T_TERM, 1 + N_COLORS);
        assert_eq!(T_ANSI, T_TERM + 3);
        assert_eq!(T_POOL, T_ANSI + 16);
        assert_eq!(ANSI_NAMES.len(), 16);
    }
}
