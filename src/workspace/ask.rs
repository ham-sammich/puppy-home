//! The interactive `ask_user_question` modal + its state.

use eframe::egui;

use crate::backend::{AskAnswer, AskOption, AskQuestion};

use super::Workspace;
use super::state::{Entry, InstanceStatus};

/// An outstanding `ask_user_question` request, rendered as a modal.
pub(crate) struct AskState {
    pub(crate) id: String,
    pub(crate) questions: Vec<AskQ>,
}

pub(crate) struct AskQ {
    pub(crate) header: String,
    pub(crate) question: String,
    pub(crate) multi_select: bool,
    pub(crate) options: Vec<AskOption>,
    pub(crate) selected: Vec<bool>,
    pub(crate) other: String,
}

impl AskState {
    pub(crate) fn from(id: String, questions: Vec<AskQuestion>) -> Self {
        let questions = questions
            .into_iter()
            .map(|q| {
                let selected = vec![false; q.options.len()];
                AskQ {
                    header: q.header,
                    question: q.question,
                    multi_select: q.multi_select,
                    options: q.options,
                    selected,
                    other: String::new(),
                }
            })
            .collect();
        AskState { id, questions }
    }
}

impl Workspace {
    /// The outstanding `ask_user_question`, if any (frontends render it).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub(crate) fn ask_state(&self) -> Option<&AskState> {
        self.pending_ask.as_ref()
    }

    /// Toggle an option (radio semantics unless the question is multi-select).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub(crate) fn ask_toggle_option(&mut self, qi: usize, oi: usize) {
        if let Some(ask) = self.pending_ask.as_mut()
            && let Some(q) = ask.questions.get_mut(qi)
            && oi < q.selected.len()
        {
            if q.multi_select {
                q.selected[oi] = !q.selected[oi];
            } else {
                for s in &mut q.selected {
                    *s = false;
                }
                q.selected[oi] = true;
            }
        }
    }

    /// Set a question's free-text "Other" answer.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub(crate) fn ask_set_other(&mut self, qi: usize, text: String) {
        if let Some(ask) = self.pending_ask.as_mut()
            && let Some(q) = ask.questions.get_mut(qi)
        {
            q.other = text;
        }
    }

    /// Submit the current selections (the modal's Submit). Frontend-agnostic:
    /// builds the answers, sends them, narrates the transcript, resumes.
    pub(crate) fn ask_submit(&mut self) {
        let Some(ask) = self.pending_ask.take() else {
            return;
        };
        let answers: Vec<AskAnswer> = ask
            .questions
            .iter()
            .map(|q| {
                let mut selected: Vec<String> = q
                    .options
                    .iter()
                    .zip(&q.selected)
                    .filter(|(_, s)| **s)
                    .map(|(o, _)| o.label.clone())
                    .collect();
                let other = q.other.trim();
                let other_text = if other.is_empty() {
                    None
                } else {
                    selected.push(other.to_string());
                    Some(other.to_string())
                };
                AskAnswer {
                    question_header: q.header.clone(),
                    selected_options: selected,
                    other_text,
                }
            })
            .collect();
        if let Some(backend) = &self.backend {
            backend.ask_response(&ask.id, &answers);
        }
        let summary: Vec<String> = ask
            .questions
            .iter()
            .map(|q| {
                let mut picks: Vec<&str> = q
                    .options
                    .iter()
                    .zip(&q.selected)
                    .filter(|(_, s)| **s)
                    .map(|(o, _)| o.label.as_str())
                    .collect();
                let other = q.other.trim();
                if !other.is_empty() {
                    picks.push(other);
                }
                format!("{}: {}", q.header, picks.join(", "))
            })
            .collect();
        self.transcript
            .push(Entry::User(format!("↳ {}", summary.join(" · "))));
        self.set_status(InstanceStatus::Running);
    }

    /// Decline the question (the modal's Cancel).
    pub(crate) fn ask_cancel(&mut self) {
        let Some(ask) = self.pending_ask.take() else {
            return;
        };
        if let Some(backend) = &self.backend {
            backend.ask_cancel(&ask.id);
        }
        self.transcript
            .push(Entry::Note("cancelled question".to_string()));
        self.set_status(InstanceStatus::Running);
    }

    pub(crate) fn render_ask_modal(&mut self, ctx: &egui::Context) {
        let title = format!("🐶 Code Puppy asks — {}", self.name);
        // 0 = nothing, 1 = submit, 2 = cancel.
        let mut action = 0u8;
        {
            let Some(ask) = self.pending_ask.as_mut() else {
                return;
            };
            egui::Window::new(title)
                .id(egui::Id::new(("ask-modal", ask.id.as_str())))
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    for q in &mut ask.questions {
                        ui.label(egui::RichText::new(&q.header).strong());
                        ui.label(&q.question);
                        ui.add_space(2.0);
                        for i in 0..q.options.len() {
                            let opt_label = q.options[i].label.clone();
                            let opt_desc = q.options[i].description.clone();
                            if q.multi_select {
                                ui.checkbox(&mut q.selected[i], &opt_label)
                                    .on_hover_text(&opt_desc);
                            } else if ui
                                .radio(q.selected[i], &opt_label)
                                .on_hover_text(&opt_desc)
                                .clicked()
                            {
                                for s in &mut q.selected {
                                    *s = false;
                                }
                                q.selected[i] = true;
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Other:").weak());
                            ui.text_edit_singleline(&mut q.other);
                        });
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Submit").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
        }

        match action {
            1 => self.ask_submit(),
            2 => self.ask_cancel(),
            _ => {}
        }
    }
}
