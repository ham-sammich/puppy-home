//! One agent card: header (avatar/identity/model pill), state line, last
//! prompt, stats row, sub-agent rows, inline steer/new-prompt input, and the
//! state-dependent action bar. Anatomy follows `pack-card.jsx`.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, Label, RichText, Sense, Stroke, vec2};

use crate::fonts::{FAMILY_GROTESK_BOLD, FAMILY_JBMONO_BOLD};
use crate::shell::ShellAction;
use crate::theme::Accents;
use crate::views::widgets;
use crate::workspace::{InstanceStatus, Workspace};

use super::{CardInput, CardState, DashboardView, InputKind, card_state, role_emoji, tilde_path};

pub(super) fn agent_card(
    ui: &mut egui::Ui,
    ws: &Workspace,
    a: &Accents,
    view: &mut DashboardView,
    actions: &mut Vec<ShellAction>,
) {
    let weak = ui.visuals().weak_text_color();
    let st = card_state(ws.status, a, weak);
    let neutral = matches!(ws.status, InstanceStatus::Idle | InstanceStatus::Starting);
    let border = if neutral {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    } else {
        st.color.linear_multiply(0.35)
    };
    let glow = st.live.then_some(st.color);
    widgets::card(ui, border, glow, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        header_row(ui, ws, &st, a, view, actions);
        state_line(ui, ws, &st, a);
        last_prompt_block(ui, ws, a);
        context_bar(ui, ws, &st, a);
        stats_row(ui, ws, a);
        sub_agent_rows(ui, ws, a);
        inline_input(ui, ws, a, view, actions);
        action_bar(ui, ws, &st, a, view, actions);
    });
}

/// Avatar + (dot, dir name, agent/path meta) + the model pill with its
/// switch-live popover.
fn header_row(
    ui: &mut egui::Ui,
    ws: &Workspace,
    st: &CardState,
    a: &Accents,
    view: &mut DashboardView,
    actions: &mut Vec<ShellAction>,
) {
    ui.horizontal(|ui| {
        widgets::avatar(ui, role_emoji(&ws.agent), a.accent, st.live);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let model = if ws.model.is_empty() {
                "model\u{2026}"
            } else {
                ws.model.as_str()
            };
            let pill = widgets::pill(ui, RichText::new(model).monospace().size(11.0))
                .on_hover_text("Switch model \u{2014} live");
            let pop = egui::Id::new(("card-model-pop", ws.id.0));
            if pill.clicked() {
                widgets::popover_toggle(ui.ctx(), pop);
            }
            widgets::popover_below(ui, pop, pill.rect, |ui| {
                ui.set_min_width(230.0);
                ui.label(RichText::new("Switch model \u{00b7} live").small().weak());
                if ws.model_catalog().is_empty() {
                    ui.weak("model catalog not loaded yet");
                }
                for m in ws.model_catalog() {
                    let sel = m.name == ws.model;
                    let resp = ui.selectable_label(sel, RichText::new(&m.name).monospace());
                    let resp = if m.description.is_empty() {
                        resp
                    } else {
                        resp.on_hover_text(&m.description)
                    };
                    if resp.clicked() && !sel {
                        actions.push(ShellAction::SetModel {
                            id: ws.id,
                            model: m.name.clone(),
                        });
                        view.toasts
                            .push(format!("{} \u{2192} {}", ws.name, m.name), a.accent);
                        widgets::popover_close(ui.ctx(), pop);
                    }
                }
            });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        widgets::status_dot(ui, st.color, st.live);
                        ui.label(
                            RichText::new(&ws.name)
                                .family(FontFamily::Name(FAMILY_GROTESK_BOLD.into()))
                                .size(15.0),
                        );
                    });
                    let agent = if ws.agent.is_empty() {
                        "agent"
                    } else {
                        ws.agent.as_str()
                    };
                    ui.add(
                        Label::new(
                            RichText::new(format!("{agent} \u{00b7} {}", tilde_path(&ws.root)))
                                .monospace()
                                .size(11.0)
                                .weak(),
                        )
                        .truncate(),
                    );
                });
            });
        });
    });
}

/// State label in its color + current tool, with the elapsed clock on the
/// right ("3:04" mid-turn, "4m ago" otherwise).
fn state_line(ui: &mut egui::Ui, ws: &Workspace, st: &CardState, a: &Accents) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(st.label).color(st.color).strong().size(12.5));
        match ws.status {
            InstanceStatus::WaitingForInput => {
                if let Some(q) = ws.pending_question() {
                    ui.add(Label::new(RichText::new(q).color(a.wait).size(11.5)).truncate())
                        .on_hover_text(q);
                }
            }
            InstanceStatus::Dead => {
                ui.label(RichText::new(&ws.status_line).color(a.error).size(11.5));
            }
            _ => {
                if st.live
                    && let Some(tool) = &ws.current_tool
                {
                    ui.label(
                        RichText::new(format!("\u{00b7} {tool}"))
                            .monospace()
                            .size(11.5)
                            .weak(),
                    );
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let now = std::time::Instant::now();
            let clock = match ws.turn_started {
                Some(t0) => widgets::fmt_elapsed(now.saturating_duration_since(t0).as_secs()),
                None => widgets::fmt_ago(now.saturating_duration_since(ws.last_activity).as_secs()),
            };
            ui.label(RichText::new(clock).monospace().size(11.5).weak());
        });
    });
}

/// Dark inset block quoting the last user prompt (one line, hover for full
/// text) with the LAST PROMPT tag + queued-steer count.
fn last_prompt_block(ui: &mut egui::Ui, ws: &Workspace, a: &Accents) {
    if ws.last_prompt.is_empty() {
        return;
    }
    ui.add_space(2.0);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::symmetric(9, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{275d}").color(a.accent).size(13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut tag = "LAST PROMPT".to_string();
                    if ws.queued_steers > 0 {
                        tag.push_str(&format!(" \u{00b7} +{} queued", ws.queued_steers));
                    }
                    ui.label(RichText::new(tag).monospace().size(9.5).weak());
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(Label::new(RichText::new(&ws.last_prompt).size(12.0)).truncate())
                            .on_hover_text(&ws.last_prompt);
                    });
                });
            });
        });
}

/// The five stat cells: tok/s (+ mini spark), tokens, tools, files, cost.
fn stats_row(ui: &mut egui::Ui, ws: &Workspace, a: &Accents) {
    ui.add_space(4.0);
    ui.columns(5, |cols| {
        stat_cell(&mut cols[0], "tok/s", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                stat_value(ui, &format!("{:.0}", ws.token_rate));
                widgets::sparkline(ui, ws.spark_history(), vec2(42.0, 14.0), a.run);
            });
        });
        stat_cell(&mut cols[1], "tokens", |ui| {
            stat_value(ui, &widgets::fmt_k(ws.total_tokens));
        });
        stat_cell(&mut cols[2], "tools", |ui| {
            stat_value(ui, &ws.tool_calls.to_string());
        });
        stat_cell(&mut cols[3], "files", |ui| {
            let (adds, dels) = ws.diff_totals();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(format!("+{adds}"))
                        .monospace()
                        .size(12.5)
                        .color(a.run),
                );
                ui.label(
                    RichText::new(format!("\u{2212}{dels}"))
                        .monospace()
                        .size(12.5)
                        .color(a.error),
                );
            });
        });
        stat_cell(&mut cols[4], "cost", |ui| {
            // null = unknown: an honest dash, never $0.00. "≈" marks values
            // priced from the sidecar's dated models.dev snapshot.
            let v = match ws.cost {
                Some(c) if ws.cost_estimated => format!("\u{2248}${c:.2}"),
                Some(c) => format!("${c:.2}"),
                None => "\u{2014}".to_string(),
            };
            stat_value(ui, &v);
        });
    });
}

/// The mock's context-progress bar: 3px, gradient think→run, live cards
/// only (matches the gpui branch). Unknown (`None`) draws nothing — a 0%
/// bar would be a lie.
fn context_bar(ui: &mut egui::Ui, ws: &Workspace, st: &CardState, a: &Accents) {
    let Some(pct) = ws.ctx_pct.filter(|_| st.live) else {
        return;
    };
    let frac = (pct / 100.0).clamp(0.0, 1.0) as f32;
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 1.5, ui.visuals().faint_bg_color);
        if frac > 0.0 {
            let mut fill = rect;
            fill.set_width((rect.width() * frac).max(3.0));
            let (c0, c1) = (a.think, a.run);
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(fill.left_top(), c0);
            mesh.colored_vertex(fill.right_top(), c1);
            mesh.colored_vertex(fill.right_bottom(), c1);
            mesh.colored_vertex(fill.left_bottom(), c0);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(egui::Shape::mesh(mesh));
        }
    }
    resp.on_hover_text(format!("context window {pct:.0}% full"));
}

fn stat_cell(ui: &mut egui::Ui, k: &str, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(RichText::new(k).small().weak());
        content(ui);
    });
}

fn stat_value(ui: &mut egui::Ui, v: &str) {
    ui.label(
        RichText::new(v)
            .family(FontFamily::Name(FAMILY_JBMONO_BOLD.into()))
            .size(12.5),
    );
}

/// Map a sub-agent's free-text status onto the accent palette.
pub(crate) fn sub_status_color(status: &str, a: &Accents) -> Color32 {
    match status {
        "running" | "thinking" | "tool_calling" | "starting" => a.think,
        "completed" | "done" | "success" => a.run,
        "error" | "failed" | "cancelled" => a.error,
        _ => Color32::GRAY,
    }
}

/// Nested `invoke_agent` rows behind a dashed left rule.
fn sub_agent_rows(ui: &mut egui::Ui, ws: &Workspace, a: &Accents) {
    if ws.sub_agents.is_empty() {
        return;
    }
    ui.add_space(4.0);
    let weak = ui.visuals().weak_text_color();
    let group = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for sa in &ws.sub_agents {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let col = sub_status_color(&sa.status, a);
                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, col);
                ui.label(
                    RichText::new(format!(
                        "\u{21b3} {} {}",
                        role_emoji(&sa.agent_name),
                        sa.agent_name
                    ))
                    .size(11.5),
                );
                ui.label(RichText::new(&sa.model_name).monospace().size(10.5).weak());
                let mut s = sa.status.clone();
                if let Some(tool) = &sa.current_tool {
                    s.push_str(&format!(" \u{00b7} {tool}"));
                }
                s.push_str(&format!(" \u{00b7} {}t", sa.tool_call_count));
                ui.label(RichText::new(s).size(11.0).color(col));
            });
        }
    });
    let r = group.response.rect;
    ui.painter().extend(egui::Shape::dashed_line(
        &[
            r.left_top() + vec2(4.0, 1.0),
            r.left_bottom() + vec2(4.0, -1.0),
        ],
        Stroke::new(1.0, weak.linear_multiply(0.5)),
        3.0,
        3.0,
    ));
}

/// The expanded steer / new-prompt input (one card at a time). Enter submits,
/// Escape closes; Steer carries the now/queue delivery toggle.
fn inline_input(
    ui: &mut egui::Ui,
    ws: &Workspace,
    a: &Accents,
    view: &mut DashboardView,
    actions: &mut Vec<ShellAction>,
) {
    if !matches!(&view.input, Some(i) if i.ws == ws.id) {
        return;
    }
    let mut submit = false;
    let mut close = false;
    {
        let inp = view.input.as_mut().expect("checked above");
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(CornerRadius::same(9))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tag = match inp.kind {
                        InputKind::Steer => "STEER",
                        InputKind::Send => "SEND",
                    };
                    ui.label(RichText::new(tag).monospace().size(9.5).color(a.accent));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let send_label = match inp.kind {
                            InputKind::Steer => "Nudge",
                            InputKind::Send => "Send",
                        };
                        if primary_btn(ui, send_label, a).clicked() {
                            submit = true;
                        }
                        if inp.kind == InputKind::Steer {
                            if ui
                                .selectable_label(inp.queue, "\u{1f4e8} queue")
                                .on_hover_text("Deliver after the current turn")
                                .clicked()
                            {
                                inp.queue = true;
                            }
                            if ui
                                .selectable_label(!inp.queue, "\u{1f3af} now")
                                .on_hover_text("Interrupt at the next model call")
                                .clicked()
                            {
                                inp.queue = false;
                            }
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let hint = match inp.kind {
                                InputKind::Steer => "Nudge this agent mid-task\u{2026}",
                                InputKind::Send => "Send a new prompt\u{2026}",
                            };
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut inp.text)
                                    .hint_text(hint)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(ui.available_width()),
                            );
                            if inp.focus {
                                resp.request_focus();
                                inp.focus = false;
                            }
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                submit = true;
                            }
                        });
                    });
                });
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
    }
    if submit {
        let (kind, text, queue) = {
            let inp = view.input.as_ref().expect("still open");
            (inp.kind, inp.text.trim().to_string(), inp.queue)
        };
        if !text.is_empty() {
            match kind {
                InputKind::Steer => {
                    let how = if queue {
                        "(queued \u{1f4e8})"
                    } else {
                        "now \u{1f3af}"
                    };
                    view.toasts
                        .push(format!("Steered {} {how}", ws.name), a.accent);
                    actions.push(ShellAction::Steer {
                        id: ws.id,
                        text,
                        queue,
                    });
                }
                InputKind::Send => {
                    view.toasts.push(format!("Sent {}", ws.name), a.accent);
                    actions.push(ShellAction::SendPrompt { id: ws.id, text });
                }
            }
        }
        close = true;
    }
    if close {
        view.input = None;
    }
}

/// State-dependent actions on the left, Changes + Open on the right.
fn action_bar(
    ui: &mut egui::Ui,
    ws: &Workspace,
    st: &CardState,
    a: &Accents,
    view: &mut DashboardView,
    actions: &mut Vec<ShellAction>,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        match ws.status {
            InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => {
                if ui.button("\u{23f8} Pause").clicked() {
                    view.toasts
                        .push(format!("{} paused at next safe point", ws.name), a.paused);
                    actions.push(ShellAction::Pause(ws.id));
                }
                stop_btn(ui, ws, a, view, actions);
                input_toggle(ui, ws, InputKind::Steer, "\u{1f3af} Steer", view);
            }
            InstanceStatus::Paused => {
                if primary_btn(ui, "\u{25b6} Resume", a).clicked() {
                    view.toasts.push(format!("{} resumed", ws.name), a.run);
                    actions.push(ShellAction::Resume(ws.id));
                }
                stop_btn(ui, ws, a, view, actions);
                input_toggle(ui, ws, InputKind::Steer, "\u{1f3af} Steer", view);
            }
            InstanceStatus::WaitingForInput => {
                if primary_btn(ui, "Answer \u{2192}", a).clicked() {
                    actions.push(ShellAction::FocusChat(ws.id));
                }
            }
            InstanceStatus::Idle => {
                input_toggle(ui, ws, InputKind::Send, "\u{2709} New prompt", view);
            }
            InstanceStatus::Dead => {
                // The honest "Retry" for a dead sidecar: relaunch + restore.
                if ui.button("\u{21bb} Restart").clicked() {
                    view.toasts
                        .push(format!("Restarting {}\u{2026}", ws.name), a.run);
                    actions.push(ShellAction::Restart(ws.id));
                }
            }
            InstanceStatus::Starting => {
                ui.label(RichText::new("warming up\u{2026}").weak().small());
            }
        }
        let _ = st;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Open \u{2192}")
                .on_hover_text("Open this workspace's chat")
                .clicked()
            {
                actions.push(ShellAction::FocusChat(ws.id));
            }
            let diffs = ws.diff_count();
            let label = if diffs > 0 {
                format!("Changes ({diffs})")
            } else {
                "Changes".to_string()
            };
            if ui
                .button(label)
                .on_hover_text("Review file changes")
                .clicked()
            {
                actions.push(ShellAction::ShowChanges(ws.id));
            }
        });
    });
}

/// Accent-filled call-to-action button (ink-on-amber).
fn primary_btn(ui: &mut egui::Ui, label: &str, a: &Accents) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(a.accent_ink))
            .fill(a.accent)
            .corner_radius(CornerRadius::same(8)),
    )
}

/// Stop button shared by live + paused states.
fn stop_btn(
    ui: &mut egui::Ui,
    ws: &Workspace,
    a: &Accents,
    view: &mut DashboardView,
    actions: &mut Vec<ShellAction>,
) {
    if ui.button("\u{23f9} Stop").clicked() {
        view.toasts.push(format!("{} stopped", ws.name), a.error);
        actions.push(ShellAction::Stop(ws.id));
    }
}

/// Toggle this card's inline input open/closed (one open card at a time).
fn input_toggle(
    ui: &mut egui::Ui,
    ws: &Workspace,
    kind: InputKind,
    label: &str,
    view: &mut DashboardView,
) {
    let open = matches!(&view.input, Some(i) if i.ws == ws.id && i.kind == kind);
    if ui.selectable_label(open, label).clicked() {
        view.input = if open {
            None
        } else {
            Some(CardInput {
                ws: ws.id,
                kind,
                text: String::new(),
                queue: false,
                focus: true,
            })
        };
    }
}
