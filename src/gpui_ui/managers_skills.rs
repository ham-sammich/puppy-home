//! Skills manager: dispatch + render (egui `views/skills_manager.rs` +
//! `views/skills_wizard.rs` at parity, dressed in the GPUI tokens).

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::managers::{F_B, F_C, F_NAME, MgrAction};
use crate::gpui_ui::managers_ui::{
    MgrArgs, act, center_hint, field, filter_field, mode_toggle, mono_block, option_card,
    paste_panel, small, step_strip, switch,
};
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{RootView, markdown};
use crate::views::common::EditMode;
use crate::views::skills_manager::{matches_filter, skill_body};
use crate::views::skills_wizard::{self, Scope, scope_for_source};
use crate::workspace::Workspace;

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl RootView {
    pub(crate) fn dispatch_skills(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        let accent = self.tokens.accent;
        match action {
            MgrAction::SkillToggle(name, on) => {
                self.mgr_pending.insert(name.clone(), on);
                self.with_ready_backend(|b| b.set_skill_enabled(&name, on));
            }
            MgrAction::SkillSelect(name) => {
                self.mgr_selected = Some(name.clone());
                self.with_ready_backend(|b| b.get_skill(&name));
            }
            MgrAction::SkillWizardOpen(edit) => {
                self.ensure_mgr_inputs(cx);
                let w = if edit {
                    // Edit needs the fetched detail (render gates the button).
                    let Some(ws) = self.serving_ws() else { return };
                    let sel = self.mgr_selected.clone();
                    match (&sel, &ws.skill_detail) {
                        (Some(sel), Some(d)) if d.name == *sel => {
                            let source = ws
                                .skills
                                .as_deref()
                                .and_then(|all| all.iter().find(|s| s.name == *sel))
                                .map(|s| s.source.clone())
                                .unwrap_or_else(|| "user".into());
                            skills_wizard::Wizard::edit(
                                &d.name,
                                &d.description,
                                skill_body(&d.content),
                                scope_for_source(&source),
                            )
                        }
                        _ => return,
                    }
                } else {
                    skills_wizard::Wizard::create()
                };
                self.seed(F_NAME, w.name.clone(), cx);
                self.seed(F_B, w.description.clone(), cx);
                self.seed(F_C, w.content.clone(), cx);
                self.skills_wizard = Some(w);
            }
            MgrAction::SkillWizardCancel => self.skills_wizard = None,
            MgrAction::SkillMode(paste) => {
                self.read_skill_inputs(cx);
                let mut seed = None;
                if let Some(w) = &mut self.skills_wizard {
                    w.error = None;
                    w.mode = if paste {
                        EditMode::Paste
                    } else {
                        EditMode::Form
                    };
                    if paste {
                        w.sync_paste_from_form();
                        seed = Some(w.paste.clone());
                    }
                }
                if let Some(text) = seed {
                    self.seed_paste(text, cx);
                }
            }
            MgrAction::SkillStep(delta) => {
                self.read_skill_inputs(cx);
                if let Some(w) = &mut self.skills_wizard {
                    w.error = None;
                    // egui gates Basics -> Content behind name + description.
                    if w.step == 0
                        && delta > 0
                        && let Err(e) = skills_wizard::validate_basics(w)
                    {
                        w.error = Some(e);
                        return;
                    }
                    w.step = (w.step as i32 + delta).clamp(0, 2) as usize;
                }
            }
            MgrAction::SkillScope(project) => {
                if let Some(w) = &mut self.skills_wizard {
                    w.scope = if project { Scope::Project } else { Scope::User };
                }
            }
            MgrAction::SkillFormat => {
                let text = self.paste_text(cx);
                let mut tidy = None;
                if let Some(w) = &mut self.skills_wizard {
                    w.paste = text;
                    match w.apply_paste() {
                        Ok(()) => {
                            w.error = None;
                            w.sync_paste_from_form();
                            tidy = Some((
                                w.paste.clone(),
                                w.name.clone(),
                                w.description.clone(),
                                w.content.clone(),
                            ));
                        }
                        Err(e) => w.error = Some(e),
                    }
                }
                if let Some((tidy, name, desc, content)) = tidy {
                    self.seed_paste(tidy, cx);
                    self.seed(F_NAME, name, cx);
                    self.seed(F_B, desc, cx);
                    self.seed(F_C, content, cx);
                }
            }
            MgrAction::SkillSubmit => {
                self.read_skill_inputs(cx);
                let paste_mode = self
                    .skills_wizard
                    .as_ref()
                    .is_some_and(|w| w.mode == EditMode::Paste);
                if paste_mode {
                    let text = self.paste_text(cx);
                    if let Some(w) = &mut self.skills_wizard {
                        w.paste = text;
                        if let Err(e) = w.apply_paste() {
                            w.error = Some(e);
                            return;
                        }
                    }
                }
                let Some(w) = &mut self.skills_wizard else {
                    return;
                };
                if let Err(e) = skills_wizard::validate_basics(w) {
                    w.error = Some(e);
                    return;
                }
                let name = w.name.trim().to_string();
                let desc = w.description.trim().to_string();
                let content = w.content.clone();
                let scope = w.scope.wire();
                self.with_ready_backend(|b| b.save_skill(&name, &desc, &content, scope));
                self.toast(format!("Saved skill {name}"), accent);
                // Show the saved skill (egui: re-list bump re-fetches detail).
                self.mgr_selected = Some(name);
                self.skills_wizard = None;
                self.mgr_last_request = None;
                self.mgr_upkeep();
            }
            _ => {}
        }
    }

    fn read_skill_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        let name = self.mgr_input_text(F_NAME, cx);
        let desc = self.mgr_input_text(F_B, cx);
        let content = self.mgr_input_text(F_C, cx);
        if let Some(w) = &mut self.skills_wizard {
            w.name = name;
            w.description = desc;
            w.content = content;
        }
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub(crate) fn body(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    if let Some(w) = args.skills_wizard {
        return wizard_body(args, w);
    }
    match &ws.skills {
        None => center_hint(&t, &["Loading skills..."]),
        Some(skills) if skills.is_empty() => center_hint(
            &t,
            &[
                "No skills found.",
                "Use \"Create skill\" to write your first SKILL.md.",
            ],
        ),
        Some(skills) => {
            let needle = args.filter.trim().to_lowercase();
            let visible: Vec<_> = skills
                .iter()
                .filter(|s| matches_filter(s, &needle))
                .collect();
            let list = div()
                .w(px(280.))
                .flex_none()
                .flex()
                .flex_col()
                .gap_1()
                .child(filter_field(args, "Filter skills..."))
                .child(if visible.is_empty() {
                    small(&t, "No skills match the filter.", t.dim).into_any_element()
                } else {
                    div()
                        .id("skill-list")
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
                                .map(|(i, s)| skill_row(args, i, s)),
                        )
                        .into_any_element()
                });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .child(list)
                .child(detail_pane(args, ws))
                .into_any_element()
        }
    }
}

/// One skill row: name (click to open) + description, source + on/off switch.
fn skill_row(args: &MgrArgs, i: usize, s: &crate::backend::SkillInfo) -> AnyElement {
    let t = args.t;
    let sel = args.selected == Some(s.name.as_str());
    let on = args.pending.get(&s.name).copied().unwrap_or(s.enabled);
    let name = s.name.clone();
    div()
        .id(("skill-row", i as u64))
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .rounded(px(7.))
        .cursor_pointer()
        .when(sel, |d| d.bg(alpha(t.accent, 0.12)))
        .hover(|d| d.bg(t.well))
        .tooltip(widgets::text_tip(s.path.clone()))
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
                        .text_color(t.text)
                        .child(s.name.clone()),
                )
                .child(small(&t, s.source.clone(), t.dim))
                .child(
                    div()
                        .id(("skill-switch", i as u64))
                        .cursor_pointer()
                        .child(switch(&t, on))
                        .tooltip(widgets::text_tip(
                            if on {
                                "Disable this skill"
                            } else {
                                "Enable this skill"
                            }
                            .into(),
                        ))
                        .on_click(act(&args.root, MgrAction::SkillToggle(s.name.clone(), !on))),
                ),
        )
        .children((!s.description.is_empty()).then(|| {
            div()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(t.weak)
                .child(s.description.clone())
        }))
        .on_click(act(&args.root, MgrAction::SkillSelect(name)))
        .into_any_element()
}

/// Read-only detail: name + Edit, path + description, markdown body.
fn detail_pane(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    let Some(selected) = args.selected else {
        return center_hint(&t, &["Select a skill to view its SKILL.md."]);
    };
    let detail_ready = ws.skill_detail.as_ref().is_some_and(|d| d.name == selected);
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(selected.to_string()),
        )
        .child(div().flex_1())
        .child(if detail_ready {
            widgets::btn(&t, "Edit")
                .id("skill-edit")
                .on_click(act(&args.root, MgrAction::SkillWizardOpen(true)))
                .into_any_element()
        } else {
            crate::gpui_ui::managers_ui::disabled_btn(&t, "Edit").into_any_element()
        });

    match &ws.skill_detail {
        Some(d) if d.name == selected => div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .child(small(&t, d.path.clone(), t.dim))
            .children((!d.description.is_empty()).then(|| small(&t, d.description.clone(), t.weak)))
            .child(
                div()
                    .id("skill-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(markdown::render(&t, skill_body(&d.content))),
            )
            .into_any_element(),
        _ => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(header)
            .child(center_hint(&t, &["Loading skill..."]))
            .into_any_element(),
    }
}

fn wizard_body(args: &MgrArgs, w: &skills_wizard::Wizard) -> AnyElement {
    let t = args.t;
    let paste = w.mode == EditMode::Paste;

    let scope_cards = |id_base: &'static str| {
        div().flex().flex_col().gap_1().children(
            [(Scope::User, false), (Scope::Project, true)]
                .into_iter()
                .enumerate()
                .map(|(i, (scope, project))| {
                    option_card(&t, w.scope == scope, scope.label(), scope.blurb())
                        .id((id_base, i as u64))
                        .on_click(act(&args.root, MgrAction::SkillScope(project)))
                        .into_any_element()
                }),
        )
    };

    let body: AnyElement = if paste {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(small(
                &t,
                "Paste a full SKILL.md (--- frontmatter --- then the markdown body):",
                t.weak,
            ))
            .child(paste_panel(args))
            .child(small(&t, "Where to save", t.weak))
            .child(scope_cards("skill-paste-scope"))
            .child(small(
                &t,
                "Format checks the syntax and tidies it; Save validates and writes it.",
                t.dim,
            ))
            .into_any_element()
    } else {
        match w.step {
            0 => div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(field(
                    &t,
                    "name \u{2014} my-skill (letters, digits, - and _)",
                    args.inputs.get(F_NAME),
                    false,
                ))
                .child(field(
                    &t,
                    "description \u{2014} one line: when should an agent reach for this?",
                    args.inputs.get(F_B),
                    false,
                ))
                .child(small(&t, "Where to save", t.weak))
                .child(scope_cards("skill-scope"))
                .children(w.editing.then(|| {
                    small(
                        &t,
                        "Saving writes <skills dir>/<name>/SKILL.md - keep the same name \
                         and scope to overwrite in place.",
                        t.dim,
                    )
                }))
                .into_any_element(),
            1 => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(field(
                    &t,
                    "SKILL.md body (markdown; the frontmatter is added on save):",
                    args.inputs.get(F_C),
                    true,
                ))
                .into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(small(&t, "Review and confirm:", t.weak))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(w.name.trim().to_string()),
                        )
                        .child(small(&t, w.scope.wire(), t.weak)),
                )
                .child(mono_block(
                    &t,
                    skills_wizard::compose_preview(&w.name, &w.description, &w.content),
                ))
                .child(small(
                    &t,
                    if w.editing {
                        "Saving overwrites the existing SKILL.md."
                    } else {
                        "The skill is discovered immediately; toggle it off any time."
                    },
                    t.dim,
                ))
                .into_any_element(),
        }
    };

    let footer = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            widgets::btn(&t, "Cancel")
                .id("skill-cancel")
                .on_click(act(&args.root, MgrAction::SkillWizardCancel)),
        )
        .child(div().flex_1())
        .children(paste.then(|| {
            widgets::btn(&t, "Format")
                .id("skill-format")
                .on_click(act(&args.root, MgrAction::SkillFormat))
        }))
        .children((!paste && w.step > 0).then(|| {
            widgets::btn(&t, "\u{2190} Back")
                .id("skill-back")
                .on_click(act(&args.root, MgrAction::SkillStep(-1)))
        }))
        .children((!paste && w.step < 2).then(|| {
            widgets::btn(&t, "Next \u{2192}")
                .id("skill-next")
                .on_click(act(&args.root, MgrAction::SkillStep(1)))
        }))
        .children((paste || w.step == 2).then(|| {
            widgets::primary_btn(&t, "\u{2713} Save skill")
                .id("skill-save")
                .on_click(act(&args.root, MgrAction::SkillSubmit))
        }));

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
                .child(small(&t, w.title(), t.text))
                .children(
                    (!paste).then(|| step_strip(&t, &["Basics", "Content", "Review"], w.step)),
                )
                .child(div().flex_1())
                .child(mode_toggle(
                    args,
                    paste,
                    MgrAction::SkillMode(false),
                    MgrAction::SkillMode(true),
                )),
        )
        .children(w.error.clone().map(|e| small(&t, e, t.error)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .id("skill-wiz-scroll")
                .overflow_y_scroll()
                .child(body),
        )
        .child(footer)
        .into_any_element()
}
