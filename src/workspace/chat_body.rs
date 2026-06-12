//! The chat region: transcript (with the empty state) + the bottom composer
//! dock + the logs panel, plus the slim terminal-mode bar. Split from
//! `view.rs` (mechanical move, no behavior change).

use eframe::egui;

use super::Workspace;
use super::render::{TurnMeta, render_entry};

impl Workspace {
    /// The chat region: transcript (scrolling) with the composer dock pinned
    /// to the bottom and the optional logs panel above it.
    pub(crate) fn render_chat_body(
        &mut self,
        ui: &mut egui::Ui,
        composer_style: &mut crate::session::ComposerStyle,
    ) {
        let id = self.id.0;

        // Bottom-pinned: the composer dock (status line + selected composer
        // style + footer; slim terminal bar while the terminal is shown).
        egui::Panel::bottom(egui::Id::new(("ws-composer", id))).show_inside(ui, |ui| {
            ui.add_space(4.0);
            self.render_dock(ui, composer_style);
            ui.add_space(4.0);
        });

        if self.show_logs {
            egui::Panel::bottom(egui::Id::new(("ws-logs", id)))
                .resizable(true)
                .default_size(120.0)
                .max_size(420.0)
                .show_inside(ui, |ui| {
                    ui.label(egui::RichText::new("sidecar logs").weak());
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .id_salt(("ws-logs-scroll", id))
                        .show(ui, |ui| {
                            for line in &self.logs {
                                ui.label(egui::RichText::new(line).monospace().small());
                            }
                        });
                });
        }

        // Main area: the embedded terminal, or the chat transcript.
        if self.show_terminal {
            self.render_terminal(ui);
        } else {
            // Immediate-mode markdown is the priciest thing we draw: every
            // entry below is re-parsed + re-laid-out EVERY frame (a ScrollArea
            // does not cull off-screen content). Long conversations made the
            // whole app sluggish -- especially on Windows -- so only the recent
            // tail renders by default; "Show older" opts into the full history.
            const TRANSCRIPT_RENDER_TAIL: usize = 120;
            let mut show_all_clicked = false;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .id_salt(("ws-transcript", id))
                .show(ui, |ui| {
                    if self.transcript_collapsed > 0 {
                        ui.weak(format!(
                            "{} earlier message(s) trimmed to keep the UI responsive.",
                            self.transcript_collapsed
                        ));
                    }
                    if self.transcript.is_empty() && self.transcript_collapsed == 0 {
                        self.render_empty_state(ui);
                    }
                    let total = self.transcript.len();
                    let start = if self.transcript_show_all {
                        0
                    } else {
                        total.saturating_sub(TRANSCRIPT_RENDER_TAIL)
                    };
                    if start > 0 {
                        ui.horizontal(|ui| {
                            ui.weak(format!("{start} older message(s) hidden for speed."));
                            if ui.small_button("Show older").clicked() {
                                show_all_clicked = true;
                            }
                        });
                    }
                    // Namespace each entry's widget ids (commonmark tables use a
                    // Grid) so repeated/duplicate content doesn't clash. Turn
                    // meta is built once per frame, not per entry.
                    let meta = TurnMeta {
                        puppy: &self.puppy_name,
                        agent: &self.agent,
                        model: &self.model,
                    };
                    for (i, entry) in self.transcript.iter().enumerate().skip(start) {
                        ui.push_id(("entry", i), |ui| {
                            render_entry(ui, entry, &mut self.md_cache, &meta);
                        });
                    }
                });
            if show_all_clicked {
                self.transcript_show_all = true;
            }
        }
    }

    /// The editor area: a tab bar of open files / Changes, then the active one.
    /// The slim bar shown while the embedded terminal fills the chat area:
    /// terminal/sessions toggles + the agent/model switchers.
    pub(crate) fn render_bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.terminal_toggle(ui);
            self.sessions_toggle(ui);
            ui.separator();
            self.agent_switcher(ui);
            self.model_switcher(ui);
        });
    }
}
