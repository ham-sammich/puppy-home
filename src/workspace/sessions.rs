//! The saved-sessions browser modal (autosave + named contexts) with preview.

use eframe::egui;

use super::Workspace;
use super::render::{AGENT_COLOR, labelled, render_markdown, short_session};
use super::state::Entry;

impl Workspace {
    /// Interactive picker for saved Code Puppy sessions (autosave + contexts).
    pub(crate) fn render_sessions_modal(&mut self, ctx: &egui::Context) {
        let id = self.id.0;
        let running = self.running;
        let puppy = self.puppy_name.clone();
        let mut do_refresh = false;
        let mut close = false;
        let mut open: Option<(String, String)> = None;
        let mut select: Option<(String, String)> = None;

        egui::Window::new(format!("🗂 Code Puppy sessions — {}", self.name))
            .id(egui::Id::new(("sessions-modal", id)))
            .collapsible(false)
            .resizable(true)
            // Anchor to the top-left with hard margins (instead of centering) so
            // the modal can never overflow off the left edge of the window.
            .anchor(egui::Align2::LEFT_TOP, [16.0, 48.0])
            .show(ctx, |ui| {
                // Size to fit *within* the window so nothing clips off-screen.
                let screen = ui.ctx().content_rect();
                let w = (screen.width() - 32.0).clamp(360.0, 1200.0);
                let h = (screen.height() - 96.0).clamp(260.0, 1000.0);
                ui.set_min_size(egui::vec2(w, h));
                ui.set_max_size(egui::vec2(w, h));
                let pane_h = (h - 150.0).max(160.0);
                ui.horizontal(|ui| {
                    ui.label("Select a saved conversation to preview it, then resume it here.");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                            do_refresh = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("{} session(s)", self.sessions.len()))
                                .weak()
                                .small(),
                        );
                    });
                });
                if running {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 180, 100),
                        "⚠ Stop the running turn before loading a session.",
                    );
                }
                ui.separator();

                ui.horizontal_top(|ui| {
                    // LEFT: search box + the (filtered) session list.
                    ui.vertical(|ui| {
                        ui.set_width(240.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.sessions_filter)
                                .desired_width(f32::INFINITY)
                                .hint_text("🔎 filter sessions…"),
                        );
                        let filter = self.sessions_filter.to_ascii_lowercase();
                        let matches = |s: &crate::backend::SessionInfo| -> bool {
                            filter.is_empty()
                                || short_session(&s.name)
                                    .to_ascii_lowercase()
                                    .contains(&filter)
                                || s.source.to_ascii_lowercase().contains(&filter)
                        };
                        let shown = self.sessions.iter().filter(|s| matches(s)).count();
                        if self.sessions.is_empty() {
                            ui.weak("No saved sessions found yet.");
                        } else if shown == 0 {
                            ui.weak("No sessions match the filter.");
                        }
                        egui::ScrollArea::vertical()
                            .id_salt(("sess-list", id))
                            .auto_shrink([false, false])
                            .max_height(pane_h)
                            .show(ui, |ui| {
                                for s in self.sessions.iter().filter(|s| matches(s)) {
                                    let is_current = s.name == self.sessions_current;
                                    let is_sel =
                                        self.selected_session.as_deref() == Some(s.name.as_str());
                                    let title = format!(
                                        "{}{}",
                                        if is_current { "● " } else { "" },
                                        short_session(&s.name)
                                    );
                                    let resp = ui.selectable_label(is_sel, title);
                                    if resp.clicked() {
                                        select = Some((s.name.clone(), s.source.clone()));
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "    {} · {} msgs · {} tok",
                                            s.source, s.messages, s.tokens
                                        ))
                                        .weak()
                                        .small(),
                                    );
                                }
                            });
                    });

                    ui.separator();

                    // RIGHT: the selected session's conversation preview.
                    ui.vertical(|ui| match self.selected_session.clone() {
                        None => {
                            ui.weak("Select a session on the left to read its conversation.");
                        }
                        Some(sel) => {
                            let source = self
                                .sessions
                                .iter()
                                .find(|s| s.name == sel)
                                .map(|s| s.source.clone())
                                .unwrap_or_else(|| "autosave".to_string());
                            let is_current = sel == self.sessions_current;
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(short_session(&sel)).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if is_current {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(120, 200, 140),
                                                "● current session",
                                            );
                                        } else if ui
                                            .add_enabled(
                                                !running,
                                                egui::Button::new("⟲ Resume this"),
                                            )
                                            .clicked()
                                        {
                                            open = Some((sel.clone(), source.clone()));
                                        }
                                    },
                                );
                            });
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .id_salt(("sess-preview", id))
                                .auto_shrink([false, false])
                                .max_height(pane_h)
                                .stick_to_bottom(false)
                                .show(ui, |ui| match &self.session_preview {
                                    Some((pname, entries)) if *pname == sel => {
                                        if entries.is_empty() {
                                            ui.weak("(no displayable messages)");
                                        }
                                        for (i, e) in entries.iter().enumerate() {
                                            ui.push_id(("pv", i), |ui| {
                                                if e.role == "user" {
                                                    let blue =
                                                        egui::Color32::from_rgb(120, 170, 255);
                                                    // Code Puppy prepends the (huge) system
                                                    // prompt onto the first user turn; collapse
                                                    // any long user message so the real
                                                    // conversation stays scannable.
                                                    if e.text.chars().count() > 600 {
                                                        let teaser: String = e
                                                            .text
                                                            .chars()
                                                            .take(80)
                                                            .collect::<String>()
                                                            .replace('\n', " ");
                                                        egui::CollapsingHeader::new(
                                                            egui::RichText::new(format!(
                                                                "you (long message): {teaser}…"
                                                            ))
                                                            .color(blue),
                                                        )
                                                        .id_salt(("pv-user", i))
                                                        .show(ui, |ui| {
                                                            ui.label(&e.text);
                                                        });
                                                        ui.add_space(6.0);
                                                    } else {
                                                        labelled(ui, "you", blue, &e.text);
                                                    }
                                                } else {
                                                    ui.colored_label(
                                                        AGENT_COLOR,
                                                        format!("🐶 {puppy}:"),
                                                    );
                                                    render_markdown(
                                                        ui,
                                                        &mut self.preview_cache,
                                                        &e.text,
                                                    );
                                                    ui.add_space(6.0);
                                                }
                                            });
                                        }
                                    }
                                    _ => {
                                        ui.weak("Loading conversation…");
                                    }
                                });
                        }
                    });
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    ui.label(egui::RichText::new("(Esc to close)").weak().small());
                });
            });

        // Safety net: Escape always closes the modal.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if do_refresh && let Some(backend) = &self.backend {
            backend.list_sessions();
        }
        if let Some((name, source)) = select {
            // New selection → clear stale preview and request this one.
            if self.selected_session.as_deref() != Some(name.as_str()) {
                self.session_preview = None;
            }
            self.selected_session = Some(name.clone());
            if let Some(backend) = &self.backend {
                backend.preview_session(&name, &source);
            }
        }
        if let Some((name, source)) = open {
            if let Some(backend) = &self.backend {
                backend.load_session(&name, &source);
            }
            self.transcript.push(Entry::Note(format!(
                "⟲ Loading session {}…",
                short_session(&name)
            )));
        }
        if close {
            self.show_sessions = false;
        }
    }
}
