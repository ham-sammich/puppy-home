//! Judge manager: dispatch + render for goal-mode verifiers (wiggum). Backed
//! by the sidecar's `list_judges` / `get_judge` / `save_judge` / `delete_judge`
//! / `toggle_judge` ops (which wrap `judge_config.py`). Mirrors the Agents
//! manager's shape — list + detail + a guided builder — but judges are a small
//! (name, model, prompt, enabled) tuple, so the builder is a single form.

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::backend::{JudgeDetail, JudgeDraft, JudgeInfo};
use crate::gpui_ui::RootView;
use crate::gpui_ui::managers::{F_D, F_E, F_NAME, MgrAction};
use crate::gpui_ui::managers_ui::{
    MgrArgs, act, center_hint, disabled_btn, field, filter_field, mono_block, small, switch,
};
use crate::gpui_ui::widgets::{self, alpha};
use crate::workspace::Workspace;

/// The standard goal-judge prompt, mirrored from wiggum's
/// `judge_config.DEFAULT_JUDGE_PROMPT` so the builder pre-fills it on create
/// (an empty prompt still falls back to it sidecar-side).
pub const DEFAULT_JUDGE_PROMPT: &str = "\
You are Code Puppy's goal-completion judge.

Decide whether the user's goal is verifiably complete based on the
implementor's latest response and (optionally) its message history.

Rules:
- You are not the implementation agent.
- Never modify files. You may use read-only tools if inspection helps.
- Never ask the user questions.
- Return the structured output exactly as requested by the runtime.
- Be strict. If completion is uncertain, mark incomplete and provide
  concrete remediation notes the implementor can act on next turn.
- For trivial conversational goals, judge based on whether the latest
  response satisfies the request.
- For coding goals, prefer concrete verification: passing tests,
  successful commands, file inspection.
";

/// The judge builder's working state (a single form: 4 fields).
#[derive(Clone, Debug)]
pub struct JudgeWizard {
    pub is_new: bool,
    /// The existing name we're editing (the lookup key for `save_judge`).
    pub original_name: String,
    pub name: String,
    pub model: String,
    pub prompt: String,
    pub enabled: bool,
    pub error: Option<String>,
}

impl JudgeWizard {
    pub fn create() -> Self {
        Self {
            is_new: true,
            original_name: String::new(),
            name: String::new(),
            model: String::new(),
            prompt: DEFAULT_JUDGE_PROMPT.to_string(),
            enabled: true,
            error: None,
        }
    }

    pub fn edit(d: &JudgeDetail) -> Self {
        Self {
            is_new: false,
            original_name: d.name.clone(),
            name: d.name.clone(),
            model: d.model.clone(),
            prompt: d.prompt.clone(),
            enabled: d.enabled,
            error: None,
        }
    }

    /// Validate name shape (mirrors `judge_config.validate_name`) + model.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.trim();
        if n.is_empty() {
            return Err("Name must not be empty.".into());
        }
        if n.len() > 64
            || !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(
                "Name must be 1\u{2013}64 chars, letters/digits/underscore/hyphen only \
                 (no spaces)."
                    .into(),
            );
        }
        if self.model.trim().is_empty() {
            return Err("Pick a model for the judge.".into());
        }
        Ok(())
    }

    /// The wire draft. `name` is the lookup key (target); `new_name` is set
    /// only when editing and the name actually changed.
    pub fn draft(&self) -> JudgeDraft {
        let name = if self.is_new {
            self.name.trim().to_string()
        } else {
            self.original_name.clone()
        };
        let new_name = if !self.is_new && self.name.trim() != self.original_name {
            self.name.trim().to_string()
        } else {
            String::new()
        };
        JudgeDraft {
            name,
            new_name,
            model: self.model.trim().to_string(),
            prompt: self.prompt.clone(),
            enabled: self.enabled,
            is_new: self.is_new,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl RootView {
    pub(crate) fn dispatch_judges(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        let accent = self.tokens.accent;
        match action {
            MgrAction::JudgeSelect(name) => {
                self.mgr_selected = Some(name.clone());
                self.judge_delete_confirm = None;
                self.with_ready_backend(|b| b.get_judge(&name));
            }
            MgrAction::JudgeToggle(name) => {
                self.with_ready_backend(|b| b.toggle_judge(&name));
                self.mgr_last_request = None; // re-list reflects the flip
                self.mgr_upkeep();
            }
            MgrAction::JudgeDelete(name) => self.judge_delete_confirm = Some(name),
            MgrAction::JudgeDeleteCancel => self.judge_delete_confirm = None,
            MgrAction::JudgeDeleteConfirm => {
                if let Some(name) = self.judge_delete_confirm.take() {
                    self.with_ready_backend(|b| b.delete_judge(&name));
                    self.toast(format!("Deleted judge {name}"), accent);
                    self.mgr_selected = None;
                    self.mgr_last_request = None;
                    self.mgr_upkeep();
                }
            }
            MgrAction::JudgeWizardOpen(edit) => {
                self.ensure_mgr_inputs(cx);
                let w = if edit {
                    let Some(ws) = self.serving_ws() else { return };
                    match (&self.mgr_selected, &ws.judge_detail) {
                        (Some(sel), Some(d)) if d.name == *sel => JudgeWizard::edit(d),
                        _ => return,
                    }
                } else {
                    JudgeWizard::create()
                };
                self.seed(F_NAME, w.name.clone(), cx);
                self.seed(F_D, w.model.clone(), cx);
                self.seed(F_E, w.prompt.clone(), cx);
                self.judge_model_menu = false;
                self.judge_wizard = Some(w);
            }
            MgrAction::JudgeWizardCancel => {
                self.judge_wizard = None;
                self.judge_model_menu = false;
            }
            MgrAction::JudgeModelMenu => self.judge_model_menu = !self.judge_model_menu,
            MgrAction::JudgeSetModel(model) => {
                self.seed(F_D, model.clone(), cx);
                if let Some(w) = &mut self.judge_wizard {
                    w.model = model;
                }
                self.judge_model_menu = false;
            }
            MgrAction::JudgeToggleEnabled => {
                if let Some(w) = &mut self.judge_wizard {
                    w.enabled = !w.enabled;
                }
            }
            MgrAction::JudgeSubmit => {
                self.read_judge_inputs(cx);
                let Some(w) = &mut self.judge_wizard else {
                    return;
                };
                if let Err(e) = w.validate() {
                    w.error = Some(e);
                    return;
                }
                let draft = w.draft();
                self.with_ready_backend(|b| b.save_judge(&draft));
                let shown = if draft.new_name.is_empty() {
                    draft.name.clone()
                } else {
                    draft.new_name.clone()
                };
                self.toast(format!("Saved judge {shown}"), accent);
                self.mgr_selected = Some(shown);
                self.judge_wizard = None;
                self.mgr_last_request = None;
                self.mgr_upkeep();
            }
            _ => {}
        }
    }

    fn read_judge_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        let name = self.mgr_input_text(F_NAME, cx);
        let prompt = self.mgr_input_text(F_E, cx);
        if let Some(w) = &mut self.judge_wizard {
            w.name = name;
            w.prompt = prompt;
            // model is owned by the dropdown (JudgeSetModel), like agents.
            w.error = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub(crate) fn body(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    if let Some(w) = args.judge_wizard {
        return wizard_body(args, ws, w);
    }
    match &ws.judges {
        None => center_hint(&t, &["Loading judges..."]),
        Some(judges) if judges.is_empty() => center_hint(
            &t,
            &[
                "No judges configured.",
                "Use \"Create judge\" to add a goal-mode verifier.",
            ],
        ),
        Some(judges) => {
            let needle = args.filter.trim().to_lowercase();
            let visible: Vec<_> = judges
                .iter()
                .filter(|j| needle.is_empty() || j.name.to_lowercase().contains(&needle))
                .collect();
            let list = div()
                .w(px(280.))
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(filter_field(args, "Filter judges..."))
                .child(if visible.is_empty() {
                    small(&t, "No judges match the filter.", t.dim).into_any_element()
                } else {
                    div()
                        .id("judge-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .children(
                            visible
                                .iter()
                                .enumerate()
                                .map(|(i, j)| judge_row(args, i, j)),
                        )
                        .into_any_element()
                });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .child(list)
                .child(detail_pane(args, ws, judges))
                .into_any_element()
        }
    }
}

/// One judge row: enable switch + name + model pill + prompt preview. The
/// switch toggles enabled; the rest selects the judge.
fn judge_row(args: &MgrArgs, i: usize, j: &JudgeInfo) -> AnyElement {
    let t = args.t;
    let sel = args.selected == Some(j.name.as_str());
    let preview = j
        .prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    div()
        .id(("judge-row", i as u64))
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(px(7.))
        .when(sel, |d| d.bg(alpha(t.accent, 0.12)))
        .hover(|d| d.bg(t.well))
        .child(
            div()
                .id(("judge-toggle", i as u64))
                .cursor_pointer()
                .child(switch(&t, j.enabled))
                .on_click(act(&args.root, MgrAction::JudgeToggle(j.name.clone()))),
        )
        .child(
            div()
                .id(("judge-name", i as u64))
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(if j.enabled { t.text } else { t.weak })
                                .child(j.name.clone()),
                        )
                        .children((!j.model.is_empty()).then(|| {
                            div()
                                .px_1()
                                .rounded(px(4.))
                                .bg(t.well)
                                .font_family("JetBrains Mono")
                                .text_size(px(9.))
                                .text_color(t.weak)
                                .child(j.model.clone())
                        })),
                )
                .children((!preview.is_empty()).then(|| {
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(10.))
                        .text_color(t.weak)
                        .child(preview)
                }))
                .on_click(act(&args.root, MgrAction::JudgeSelect(j.name.clone()))),
        )
        .into_any_element()
}

/// Read-only detail: header + Edit/Delete, then model/enabled + the prompt.
fn detail_pane(args: &MgrArgs, ws: &Workspace, judges: &[JudgeInfo]) -> AnyElement {
    let t = args.t;
    let Some(selected) = args.selected else {
        return center_hint(&t, &["Select a judge to view its config."]);
    };
    let row = judges.iter().find(|j| j.name == selected);
    let detail_ready = ws.judge_detail.as_ref().is_some_and(|d| d.name == selected);
    let deleting = args.judge_delete_confirm == Some(selected);

    let header = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(selected.to_string()),
        )
        .child(div().flex_1())
        .child(if detail_ready {
            widgets::btn(&t, "Edit")
                .id("judge-edit")
                .on_click(act(&args.root, MgrAction::JudgeWizardOpen(true)))
                .into_any_element()
        } else {
            disabled_btn(&t, "Edit").into_any_element()
        })
        .child(widgets::btn(&t, "Delete").id("judge-delete").on_click(act(
            &args.root,
            MgrAction::JudgeDelete(selected.to_string()),
        )));

    let confirm = deleting.then(|| {
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(small(&t, format!("Delete judge \"{selected}\"?"), t.paused))
            .child(
                widgets::btn(&t, "Yes, delete")
                    .border_color(alpha(t.error, 0.8))
                    .id("judge-delete-yes")
                    .on_click(act(&args.root, MgrAction::JudgeDeleteConfirm)),
            )
            .child(
                widgets::btn(&t, "Cancel")
                    .id("judge-delete-no")
                    .on_click(act(&args.root, MgrAction::JudgeDeleteCancel)),
            )
    });

    let mut col = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .children(confirm);
    if let Some(r) = row {
        col = col.child(small(
            &t,
            if r.enabled { "enabled" } else { "disabled" }.to_string(),
            if r.enabled { t.accent } else { t.dim },
        ));
    }

    match &ws.judge_detail {
        Some(d) if d.name == selected => col
            .child(small(&t, format!("Model: {}", d.model), t.text))
            .child(small(&t, "Judge prompt", t.dim))
            .child(
                div()
                    .id("judge-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(mono_block(&t, d.prompt.clone())),
            )
            .into_any_element(),
        _ => col
            .child(center_hint(&t, &["Loading judge..."]))
            .into_any_element(),
    }
}

/// The guided builder: name + model dropdown + prompt + enabled toggle.
fn wizard_body(args: &MgrArgs, ws: &Workspace, w: &JudgeWizard) -> AnyElement {
    let t = args.t;
    let title = if w.is_new {
        "New judge".to_string()
    } else {
        format!("Edit judge \"{}\"", w.original_name)
    };
    let enabled_row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(small(&t, "enabled", t.weak))
        .child(
            div()
                .id("judge-enabled")
                .cursor_pointer()
                .child(switch(&t, w.enabled))
                .on_click(act(&args.root, MgrAction::JudgeToggleEnabled)),
        );

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(title),
                )
                .child(div().flex_1())
                .child(enabled_row),
        )
        .children(w.error.as_ref().map(|e| small(&t, e.clone(), t.error)))
        .child(field(&t, "name", args.inputs.get(F_NAME), false))
        .child(model_dropdown(args, ws, w))
        .child(div().flex_1().min_h_0().child(field(
            &t,
            "judge prompt",
            args.inputs.get(F_E),
            true,
        )))
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(
                    widgets::primary_btn(
                        &t,
                        if w.is_new {
                            "Create judge"
                        } else {
                            "Save judge"
                        },
                    )
                    .id("judge-submit")
                    .on_click(act(&args.root, MgrAction::JudgeSubmit)),
                )
                .child(
                    widgets::btn(&t, "Cancel")
                        .id("judge-cancel")
                        .on_click(act(&args.root, MgrAction::JudgeWizardCancel)),
                ),
        )
        .into_any_element()
}

/// Model picker reusing the workspace's model catalog (same as the agent
/// builder's dropdown, minus the "global default" row — a judge needs a model).
fn model_dropdown(args: &MgrArgs, ws: &Workspace, w: &JudgeWizard) -> AnyElement {
    let t = args.t;
    let catalog = ws.model_catalog();
    let cur = w.model.trim().to_string();
    let open = args.judge_model_menu;
    let label = if cur.is_empty() {
        "pick a model\u{2026}".to_string()
    } else {
        cur.clone()
    };
    let button = div()
        .id("judge-model-dd")
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(if open {
            alpha(t.accent, 0.7)
        } else {
            t.line_soft
        })
        .cursor_pointer()
        .hover(|d| d.border_color(alpha(t.accent, 0.5)))
        .child(
            div()
                .flex_1()
                .font_family("JetBrains Mono")
                .text_size(px(11.5))
                .text_color(if cur.is_empty() { t.dim } else { t.text })
                .child(label),
        )
        .child(small(
            &t,
            if open { "\u{25b4}" } else { "\u{25be}" },
            t.weak,
        ))
        .on_click(act(&args.root, MgrAction::JudgeModelMenu));

    let mut col = div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(small(&t, "model", t.weak))
        .child(button);
    if open {
        let mut menu = div()
            .id("judge-model-menu")
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(200.))
            .overflow_y_scroll()
            .p_1()
            .rounded(px(8.))
            .bg(t.card)
            .border_1()
            .border_color(t.line_soft);
        for (i, m) in catalog.iter().enumerate() {
            let on = cur == m.name;
            menu = menu.child(
                div()
                    .id(("judge-model-opt", i as u64))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(alpha(t.accent, 0.16)))
                    .hover(|d| d.bg(t.well))
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .text_color(if on { t.accent } else { t.text })
                    .child(m.name.clone())
                    .on_click(act(&args.root, MgrAction::JudgeSetModel(m.name.clone()))),
            );
        }
        if catalog.is_empty() {
            menu = menu.child(small(&t, "No models reported by the sidecar yet.", t.dim));
        }
        col = col.child(menu);
    }
    col.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_bad_names_and_missing_model() {
        let mut w = JudgeWizard::create();
        w.name = "tests pass".into(); // space -> invalid
        w.model = "gpt-5".into();
        assert!(w.validate().is_err());
        w.name = "tests-pass".into();
        assert!(w.validate().is_ok());
        w.model = "  ".into();
        assert!(w.validate().is_err());
        w.name = String::new();
        w.model = "gpt-5".into();
        assert!(w.validate().is_err());
    }

    #[test]
    fn draft_sets_new_name_only_on_rename() {
        // Create: name is the target, no rename, is_new true.
        let mut w = JudgeWizard::create();
        w.name = "fresh".into();
        w.model = "gpt-5".into();
        let d = w.draft();
        assert!(d.is_new);
        assert_eq!(d.name, "fresh");
        assert_eq!(d.new_name, "");

        // Edit, unchanged name: target = original, no rename.
        let detail = JudgeDetail {
            name: "old".into(),
            model: "gpt-5".into(),
            prompt: "p".into(),
            enabled: true,
        };
        let mut e = JudgeWizard::edit(&detail);
        let d = e.draft();
        assert!(!d.is_new);
        assert_eq!(d.name, "old");
        assert_eq!(d.new_name, "");

        // Edit + rename: target = original key, new_name = the new label.
        e.name = "renamed".into();
        let d = e.draft();
        assert_eq!(d.name, "old");
        assert_eq!(d.new_name, "renamed");
    }

    #[test]
    fn create_prefills_default_prompt() {
        let w = JudgeWizard::create();
        assert!(
            w.prompt
                .starts_with("You are Code Puppy's goal-completion judge.")
        );
        assert!(w.enabled);
    }
}
