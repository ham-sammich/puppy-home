//! The composer dock (Workspace Chat redesign): status line, the four
//! composer skins over the shared core in `composer.rs`, the agent/model
//! switcher popovers, the style-preference footer, and the empty state.
//!
//! Perf: the only decorative loop here is the empty-state bob, gated on
//! `widgets::motion_enabled` (reduce-motion preference + window focus) and
//! driven by a bounded `request_repaint_after`.

use std::time::Duration;

use eframe::egui::{self, Align2, CornerRadius, FontFamily, FontId, RichText, Sense, vec2};

use crate::fonts::FAMILY_GROTESK_BOLD;
use crate::session::ComposerStyle;
use crate::views::dashboard::role_emoji;
use crate::views::widgets;
use crate::workspace::InstanceStatus;

use super::Workspace;
use super::composer::ComposerFx;

/// Starter prompts for the Guided composer (label with glyph, prompt sent).
const STARTERS: [(&str, &str); 4] = [
    ("\u{1f4c2} Explain this repo", "Explain this repo"),
    ("\u{1f41b} Fix the failing tests", "Fix the failing tests"),
    ("\u{2728} Add a feature", "Add a feature"),
    ("\u{1f4dd} Write a README", "Write a README"),
];

/// Status-line vocabulary: label + whether the dot should pulse.
fn status_chip(s: InstanceStatus) -> (&'static str, bool) {
    match s {
        InstanceStatus::Starting => ("Starting", false),
        InstanceStatus::Idle => ("Ready", false),
        InstanceStatus::Running => ("Working", true),
        InstanceStatus::Thinking => ("Thinking", true),
        InstanceStatus::ToolCalling => ("Tool running", true),
        InstanceStatus::WaitingForInput => ("Needs you", false),
        InstanceStatus::Paused => ("Paused", false),
        InstanceStatus::Dead => ("Stopped", false),
    }
}

impl Workspace {
    /// The whole bottom dock: status line, then the composer (or the pending
    /// interactive prompt, or the slim terminal bar), then the footer.
    pub(crate) fn render_dock(&mut self, ui: &mut egui::Ui, style: &mut ComposerStyle) {
        self.dock_status_line(ui);
        ui.add_space(4.0);
        if self.show_terminal {
            self.render_bottom_bar(ui);
            return;
        }
        if self.pending.is_some() {
            self.render_pending(ui);
        } else {
            let apply = self.composer_prelude(ui);
            let fx = match *style {
                ComposerStyle::Classic => self.render_style_classic(ui, apply),
                ComposerStyle::Unified => self.render_style_unified(ui, apply),
                ComposerStyle::Palette => self.render_style_palette(ui, apply),
                ComposerStyle::Guided => self.render_style_guided(ui, apply),
            };
            self.composer_epilogue(ui, apply, fx);
        }
        self.dock_footer(ui, style);
    }

    /// "● Ready · code-puppy · model" + the running-turn controls (steer
    /// delivery toggle, pause/resume, stop) on the right — shared by every
    /// style so no skin has to duplicate them.
    fn dock_status_line(&mut self, ui: &mut egui::Ui) {
        let (label, live) = status_chip(self.status);
        let color = self.status.color();
        ui.horizontal(|ui| {
            widgets::status_dot(ui, color, live && !self.paused);
            ui.label(RichText::new(label).color(color).small());
            ui.label(RichText::new("\u{00b7}").weak().small());
            ui.label(
                RichText::new(if self.agent.is_empty() {
                    "agent"
                } else {
                    &self.agent
                })
                .strong()
                .small(),
            );
            if !self.model.is_empty() {
                ui.label(RichText::new(&self.model).monospace().weak().small());
            }
            if self.running {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{23f9}")
                        .on_hover_text("Cancel the running turn")
                        .clicked()
                    {
                        if let Some(backend) = &self.backend {
                            backend.cancel();
                        }
                        self.status_line = "Cancelling\u{2026}".to_string();
                    }
                    let pause_label = if self.paused { "\u{25b6}" } else { "\u{23f8}" };
                    if ui
                        .small_button(pause_label)
                        .on_hover_text("Pause/resume the turn at the next safe point")
                        .clicked()
                    {
                        self.toggle_pause();
                    }
                    let mode = if self.steer_queue_mode {
                        "\u{1f4e8} queue"
                    } else {
                        "\u{1f3af} now"
                    };
                    if ui
                        .selectable_label(false, RichText::new(mode).small())
                        .on_hover_text(
                            "Steer delivery \u{2014} now: interrupt mid-turn \u{00b7} \
                             queue: after this turn",
                        )
                        .clicked()
                    {
                        self.steer_queue_mode = !self.steer_queue_mode;
                    }
                });
            }
        });
    }

    /// Footer: keyboard hint + the  Composer style-preference popover.
    fn dock_footer(&mut self, ui: &mut egui::Ui, style: &mut ComposerStyle) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let hint = match *style {
                ComposerStyle::Unified | ComposerStyle::Guided => {
                    "\u{23ce} send \u{00b7} \u{21e7}\u{23ce} newline"
                }
                ComposerStyle::Classic | ComposerStyle::Palette => "\u{23ce} send",
            };
            ui.label(RichText::new(hint).weak().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui
                    .small_button(format!("\u{2699} Composer: {}", style.label()))
                    .on_hover_text(
                        "Composer style \u{2014} your preference, saved on this machine",
                    );
                let id = egui::Id::new(("composer-style-pop", self.id.0));
                if resp.clicked() {
                    widgets::popover_toggle(ui.ctx(), id);
                }
                widgets::popover_above(ui, id, resp.rect, |ui| {
                    ui.set_min_width(250.0);
                    ui.label(RichText::new("Composer style").strong());
                    ui.label(
                        RichText::new("Saved on this machine \u{2014} applies to every workspace.")
                            .weak()
                            .small(),
                    );
                    ui.add_space(3.0);
                    for s in ComposerStyle::ALL {
                        let row = option_row(ui, s.label(), s.description(), *style == s);
                        if row.clicked() {
                            *style = s;
                            widgets::popover_close(ui.ctx(), id);
                        }
                    }
                });
            });
        });
    }

    // ---------------------------------------------------------------- styles

    /// Unified: one rounded accent-border bar — growing textarea, send, then
    /// agent/model pills + Paste / @File / Terminal inline.
    fn render_style_unified(&mut self, ui: &mut egui::Ui, apply: bool) -> ComposerFx {
        let mut fx = ComposerFx::default();
        // Consume Enter before the field exists so it sends (⇧⏎ newlines).
        if !apply && self.take_enter(ui) {
            fx.submit = true;
        }
        let accent = ui.visuals().hyperlink_color;
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(egui::Stroke::new(1.5, accent.linear_multiply(0.8)))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let hint = self.composer_hint();
                    let input_id = self.composer_input_id();
                    let field = ui.add_enabled(
                        self.ready,
                        egui::TextEdit::multiline(&mut self.input)
                            .id(input_id)
                            .desired_rows(2)
                            .desired_width((ui.available_width() - 86.0).max(80.0))
                            .hint_text(hint),
                    );
                    self.focus_if_requested(&field);
                    let label = if self.running {
                        "Steer \u{23ce}"
                    } else {
                        "Send \u{23ce}"
                    };
                    if ui
                        .add_enabled(self.ready, egui::Button::new(label))
                        .clicked()
                    {
                        fx.submit = true;
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    self.agent_switcher(ui);
                    self.model_switcher(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.terminal_toggle(ui);
                        if ui
                            .button("@ File")
                            .on_hover_text("Browse workspace files and @-reference one")
                            .clicked()
                        {
                            fx.files = true;
                        }
                        if ui
                            .button("\u{1f5bc} Paste")
                            .on_hover_text("Attach an image from the clipboard")
                            .clicked()
                        {
                            fx.paste = true;
                        }
                    });
                });
            });
        fx
    }

    /// Palette: mono  prompt, ⌘K opens the slash palette, keyboard hints.
    fn render_style_palette(&mut self, ui: &mut egui::Ui, apply: bool) -> ComposerFx {
        let mut fx = ComposerFx::default();
        // ⌘K (Ctrl+K): force the slash palette open by seeding a "/" query.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
            if !self.input.starts_with('/') {
                self.input.insert(0, '/');
            }
            self.request_input_focus = true;
        }
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(CornerRadius::same(11))
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("\u{276f}")
                            .color(ui.visuals().hyperlink_color)
                            .monospace()
                            .strong(),
                    );
                    let input_id = self.composer_input_id();
                    let field = ui.add_enabled(
                        self.ready,
                        egui::TextEdit::singleline(&mut self.input)
                            .id(input_id)
                            .font(egui::TextStyle::Monospace)
                            .frame(egui::Frame::new()) // seamless inside our bar
                            .desired_width((ui.available_width() - 92.0).max(60.0))
                            .hint_text("Run a command or describe a task\u{2026}"),
                    );
                    self.focus_if_requested(&field);
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !apply && enter {
                        fx.submit = true;
                    }
                    ui.label(
                        RichText::new("\u{2318}K palette")
                            .monospace()
                            .weak()
                            .small(),
                    );
                });
            });
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "\u{2318}K commands \u{00b7} / slash \u{00b7} @ files \u{00b7} \u{23ce} send",
            )
            .monospace()
            .weak()
            .small(),
        );
        fx
    }

    /// Guided: starter-prompt chips, a paste/drop hint zone, labeled
    /// agent/model selectors, and a big "Send to {puppy} →".
    fn render_style_guided(&mut self, ui: &mut egui::Ui, apply: bool) -> ComposerFx {
        let mut fx = ComposerFx::default();
        if !self.running && self.input.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (label, prompt) in STARTERS {
                    if widgets::pill(ui, label).clicked() {
                        self.input = prompt.to_string();
                        fx.submit = true;
                    }
                }
            });
            ui.add_space(6.0);
        }
        if !apply && self.take_enter(ui) {
            fx.submit = true;
        }
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(12.0)
            .show(ui, |ui| {
                let hint = format!("Describe what you'd like {} to do\u{2026}", self.puppy_name);
                let input_id = self.composer_input_id();
                let field = ui.add_enabled(
                    self.ready,
                    egui::TextEdit::multiline(&mut self.input)
                        .id(input_id)
                        .desired_rows(2)
                        .desired_width(ui.available_width())
                        .hint_text(hint),
                );
                self.focus_if_requested(&field);
                ui.add_space(6.0);
                // Paste zone (drag-drop of image files is a known gap; the
                // clipboard path is the one that's wired end-to-end).
                let zone = egui::Frame::new()
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "\u{1f5bc} Paste a screenshot (Ctrl+V) or click to attach \
                                     from the clipboard \u{2014} show {} what you mean",
                                    self.puppy_name
                                ))
                                .weak()
                                .small(),
                            );
                        });
                    })
                    .response
                    .interact(Sense::click());
                if zone
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    fx.paste = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Who should help?").weak().small());
                        self.agent_switcher(ui);
                    });
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Brain").weak().small());
                        self.model_switcher(ui);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!("Send to {} \u{2192}", self.puppy_name);
                        let btn = egui::Button::new(
                            RichText::new(label).color(ui.visuals().extreme_bg_color),
                        )
                        .fill(ui.visuals().hyperlink_color)
                        .corner_radius(CornerRadius::same(9));
                        if ui.add_enabled(self.ready, btn).clicked() {
                            fx.submit = true;
                        }
                    });
                });
            });
        fx
    }

    // ------------------------------------------------------------- switchers

    /// Agent pill → popover with descriptions; tap to `set_agent` (no typing).
    pub(crate) fn agent_switcher(&mut self, ui: &mut egui::Ui) {
        let name = if self.agent.is_empty() {
            "agent"
        } else {
            self.agent.as_str()
        };
        let resp = widgets::pill(ui, format!("{} {name} \u{25be}", role_emoji(&self.agent)))
            .on_hover_text("Switch agent \u{2014} no typing");
        let id = egui::Id::new(("agent-switcher", self.id.0));
        if resp.clicked() {
            widgets::popover_toggle(ui.ctx(), id);
        }
        let mut chosen: Option<String> = None;
        widgets::popover_above(ui, id, resp.rect, |ui| {
            ui.set_min_width(280.0);
            ui.label(RichText::new("Switch agent").weak().small());
            if self.agents.is_empty() {
                ui.weak("agent catalog not loaded yet");
            }
            for a in &self.agents {
                let label = if a.display_name.is_empty() {
                    &a.name
                } else {
                    &a.display_name
                };
                let title = format!("{} {label}", role_emoji(&a.name));
                if option_row(ui, &title, &a.description, a.current).clicked() && !a.current {
                    chosen = Some(a.name.clone());
                    widgets::popover_close(ui.ctx(), id);
                }
            }
        });
        if let Some(name) = chosen
            && let Some(backend) = &self.backend
        {
            backend.set_agent(&name);
        }
    }

    /// Model pill (mono) → popover with descriptions; tap to `set_model`.
    pub(crate) fn model_switcher(&mut self, ui: &mut egui::Ui) {
        let current = if self.model.is_empty() {
            "model\u{2026}"
        } else {
            self.model.as_str()
        };
        let resp = widgets::pill(
            ui,
            RichText::new(format!("{current} \u{25be}"))
                .monospace()
                .size(11.5),
        )
        .on_hover_text("Switch model \u{2014} live");
        let id = egui::Id::new(("model-switcher", self.id.0));
        if resp.clicked() {
            widgets::popover_toggle(ui.ctx(), id);
        }
        let mut chosen: Option<String> = None;
        widgets::popover_above(ui, id, resp.rect, |ui| {
            ui.set_min_width(250.0);
            ui.label(RichText::new("Switch model").weak().small());
            if self.models.is_empty() {
                ui.weak("model catalog not loaded yet");
            }
            for m in &self.models {
                if option_row(ui, &m.name, &m.description, m.current).clicked() && !m.current {
                    chosen = Some(m.name.clone());
                    widgets::popover_close(ui.ctx(), id);
                }
            }
        });
        if let Some(name) = chosen
            && let Some(backend) = &self.backend
        {
            backend.set_model(&name);
        }
    }

    // --------------------------------------------------------------- toggles

    /// The embedded-terminal toggle (shared by Classic, Unified, terminal bar).
    pub(crate) fn terminal_toggle(&mut self, ui: &mut egui::Ui) {
        let term_live = self.terminal.as_ref().map(|t| t.alive).unwrap_or(false);
        let label = if self.show_terminal {
            "\u{1f5a5} Terminal \u{25be}"
        } else {
            "\u{1f5a5} Terminal"
        };
        let resp = ui
            .selectable_label(self.show_terminal, label)
            .on_hover_text("Toggle an embedded shell in the chat area");
        if resp.clicked() {
            self.show_terminal = !self.show_terminal;
            if self.show_terminal && self.terminal.is_none() {
                self.spawn_terminal(ui.ctx().clone());
            }
        }
        if self.show_terminal && !term_live && self.terminal.is_some() {
            ui.colored_label(egui::Color32::from_gray(150), "(exited)");
        }
    }

    /// The session-browser toggle (shared by Classic + the terminal bar).
    pub(crate) fn sessions_toggle(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(self.show_sessions, "\u{1f5c2} Sessions")
            .on_hover_text("Browse & resume saved Code Puppy conversations")
            .clicked()
        {
            self.show_sessions = !self.show_sessions;
            if self.show_sessions
                && let Some(backend) = &self.backend
            {
                backend.list_sessions();
            }
        }
    }

    // ------------------------------------------------------------ empty state

    /// Centered breathing puppy + floating "z z z" when the transcript is
    /// empty. The bob runs only while visible, focused, and motion is allowed;
    /// the stage rect is fixed so breathing never reflows the layout.
    pub(crate) fn render_empty_state(&self, ui: &mut egui::Ui) {
        ui.add_space((ui.available_height() * 0.5 - 110.0).max(16.0));
        ui.vertical_centered(|ui| {
            let animate = widgets::motion_enabled(ui.ctx());
            let phase = if animate {
                (ui.input(|i| i.time) * std::f64::consts::TAU / 4.2).sin() as f32
            } else {
                0.0
            };
            let (rect, _) = ui.allocate_exact_size(vec2(140.0, 84.0), Sense::hover());
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                let size = 54.0 * (1.0 + 0.035 * phase);
                p.text(
                    rect.center() + vec2(0.0, 2.0 - 2.0 * phase),
                    Align2::CENTER_CENTER,
                    "\u{1f436}",
                    FontId::proportional(size),
                    ui.visuals().text_color(),
                );
                let weak = ui.visuals().weak_text_color();
                for i in 0..3u8 {
                    let f = i as f32;
                    p.text(
                        rect.center() + vec2(36.0 + f * 8.0, -16.0 - f * 8.0 - 3.0 * phase),
                        Align2::CENTER_CENTER,
                        "z",
                        FontId::proportional(10.0 + f * 2.0),
                        weak.linear_multiply(0.45 + 0.15 * phase),
                    );
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("How can {} help you?", self.puppy_name))
                    .family(FontFamily::Name(FAMILY_GROTESK_BOLD.into()))
                    .size(20.0),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Send a message, paste a screenshot, or type / for a command.")
                    .weak(),
            );
            if animate {
                ui.ctx().request_repaint_after(Duration::from_millis(66));
            }
        });
    }
}

/// A two-line popover row (title + weak description) that's clickable as a
/// whole. Selected rows get an "active" tag.
fn option_row(ui: &mut egui::Ui, title: &str, desc: &str, selected: bool) -> egui::Response {
    let resp = ui
        .scope(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                if selected {
                    ui.label(
                        RichText::new("active")
                            .small()
                            .color(ui.visuals().hyperlink_color),
                    );
                }
            });
            if !desc.is_empty() {
                ui.label(RichText::new(desc).weak().small());
            }
        })
        .response
        .interact(Sense::click());
    ui.add_space(3.0);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
