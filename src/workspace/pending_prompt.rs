//! Inline interactive prompts: when Code Puppy asks the user something mid-turn
//! (a text input, a confirm, or a single-choice select), this renders the reply
//! widget beneath the transcript. Distinct from composing a normal message.

use eframe::egui;

use super::Workspace;
use super::state::PendingKind;

impl Workspace {
    pub(crate) fn render_pending(&mut self, ui: &mut egui::Ui) {
        let mut submit = false;
        if let Some(pending) = &mut self.pending {
            match &pending.kind {
                PendingKind::Input { prompt, password } => {
                    ui.label(egui::RichText::new(prompt).strong());
                    ui.horizontal(|ui| {
                        let edit = egui::TextEdit::singleline(&mut pending.text)
                            .desired_width(ui.available_width() - 80.0)
                            .password(*password);
                        let field = ui.add(edit);
                        let enter =
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Reply").clicked() || enter {
                            submit = true;
                        }
                    });
                }
                PendingKind::Confirm {
                    title,
                    description,
                    options,
                } => {
                    ui.label(egui::RichText::new(title).strong());
                    if !description.is_empty() {
                        ui.label(description);
                    }
                    ui.horizontal(|ui| {
                        for (i, opt) in options.iter().enumerate() {
                            if ui.button(opt).clicked() {
                                pending.selection = i;
                                submit = true;
                            }
                        }
                    });
                }
                PendingKind::Select { prompt, options } => {
                    ui.label(egui::RichText::new(prompt).strong());
                    for (i, opt) in options.iter().enumerate() {
                        if ui.selectable_label(pending.selection == i, opt).clicked() {
                            pending.selection = i;
                        }
                    }
                    if ui.button("Select").clicked() {
                        submit = true;
                    }
                }
            }
        }
        if submit {
            self.answer_pending();
        }
    }
}
