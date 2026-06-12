//! Den roster: members grouped (avatar, you/host tags, puppy, presence) with
//! their compact RoomAgent cards (state, model, tok/s, verb + file, diff
//! counts, a short remote sparkline, Open / Nudge actions).

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, vec2};
use puppy_relay::protocol::{Presence, RoomAgentInfo};

use crate::shell::ShellAction;
use crate::supervisor::Supervisor;
use crate::theme::Accents;
use crate::views::dashboard::role_emoji;
use crate::views::widgets::{self, Toasts};

use super::Conn;

/// Map a roster state string (an `InstanceStatus::label()`) onto the accents.
fn state_color(state: &str, a: &Accents, weak: Color32) -> Color32 {
    match state {
        "running" => a.run,
        "thinking" | "tool" | "starting" => a.think,
        "waiting for input" => a.wait,
        "paused" => a.paused,
        "dead" => a.error,
        _ => weak,
    }
}

#[allow(clippy::too_many_arguments)] // a render seam, not an API
pub(super) fn render(
    ui: &mut egui::Ui,
    conn: &mut Conn,
    sup: &Supervisor,
    accents: &Accents,
    actions: &mut Vec<ShellAction>,
    toasts: &mut Toasts,
    me: &str,
    my_puppy: &str,
) {
    let Conn {
        client,
        den,
        sparks,
        claims,
        ..
    } = conn;
    let weak = ui.visuals().weak_text_color();

    for m in &den.members {
        let color = crate::theme::parse_hex(&m.color).unwrap_or(accents.accent);
        // Member header: avatar ring, name + tags, puppy, presence.
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                p.circle_filled(rect.center(), 12.0, ui.visuals().faint_bg_color);
                p.circle_stroke(rect.center(), 12.0, Stroke::new(1.5, color));
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "\u{1f436}",
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
            ui.label(RichText::new(&m.user).strong());
            if m.user == me {
                tag(ui, "you", accents.accent);
            }
            if m.host {
                tag(ui, "host", weak);
            }
            if !m.puppy.is_empty() {
                ui.label(
                    RichText::new(format!("\u{1f436} {}", m.puppy))
                        .color(color)
                        .small(),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, pcolor) = match m.presence {
                    Presence::Active => ("active", accents.run),
                    Presence::Idle => ("idle", weak),
                };
                ui.label(RichText::new(label).color(pcolor).small());
                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, pcolor);
            });
        });

        // Their agent cards (from the latest roster broadcast).
        match den.roster.get(&m.user) {
            Some((agents, _)) if !agents.is_empty() => {
                for a in agents {
                    let spark = sparks
                        .get(&m.user)
                        .and_then(|rings| rings.get(&a.dir))
                        .map(|r| r.samples());
                    room_agent(
                        ui,
                        a,
                        color,
                        spark,
                        accents,
                        weak,
                        m.user == me,
                        me,
                        my_puppy,
                        client,
                        sup,
                        actions,
                        toasts,
                    );
                }
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.add_space(34.0);
                    ui.label(RichText::new("no puppy spun up here yet").weak().small());
                });
            }
        }
        ui.add_space(10.0);
    }

    // Active file claims (agent-side coordination) — kept from the classic
    // panel, collapsed: the mock has no surface for them.
    if !claims.is_empty() {
        egui::CollapsingHeader::new(format!("File claims ({})", claims.len()))
            .default_open(false)
            .show(ui, |ui| {
                for c in claims.iter() {
                    let who = if c.puppy.is_empty() {
                        c.user.clone()
                    } else {
                        format!("{} ({})", c.user, c.puppy)
                    };
                    let line = if c.note.is_empty() {
                        format!("{} \u{2014} {who}", c.path)
                    } else {
                        format!("{} \u{2014} {who}: {}", c.path, c.note)
                    };
                    ui.label(RichText::new(line).monospace().small());
                }
            });
    }
}

fn tag(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color.linear_multiply(0.6)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(9.5));
        });
}

/// One compact agent card: owner-colored left rule, state dot, agent + model,
/// tok/s, verb + file, +A −D, sparkline, Open / Nudge.
#[allow(clippy::too_many_arguments)] // a render seam, not an API
fn room_agent(
    ui: &mut egui::Ui,
    a: &RoomAgentInfo,
    owner: Color32,
    spark: Option<&[f32]>,
    accents: &Accents,
    weak: Color32,
    mine: bool,
    me: &str,
    my_puppy: &str,
    client: &crate::pack::PackClient,
    sup: &Supervisor,
    actions: &mut Vec<ShellAction>,
    toasts: &mut Toasts,
) {
    let st = state_color(&a.state, accents, weak);
    ui.horizontal(|ui| {
        ui.add_space(34.0);
        let resp = egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 8.0).max(240.0));
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.horizontal(|ui| {
                    widgets::status_dot(ui, st, false);
                    ui.label(
                        RichText::new(format!("{} {}", role_emoji(&a.agent), a.agent)).size(12.5),
                    );
                    if !a.model.is_empty() {
                        ui.label(RichText::new(&a.model).monospace().weak().size(10.5));
                    }
                    ui.label(RichText::new(&a.dir).monospace().weak().size(10.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if a.tps > 0.5 {
                            ui.label(
                                RichText::new(format!("{:.0} t/s", a.tps))
                                    .monospace()
                                    .color(accents.run)
                                    .size(11.0),
                            );
                        } else {
                            ui.label(RichText::new(&a.state).weak().size(11.0));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if !a.verb.is_empty() {
                        ui.label(RichText::new(&a.verb).color(st).size(11.0));
                    }
                    if !a.file.is_empty() {
                        ui.label(RichText::new(&a.file).monospace().weak().size(11.0));
                    }
                    if a.added + a.removed > 0 {
                        ui.label(
                            RichText::new(format!("+{}", a.added))
                                .monospace()
                                .color(accents.run)
                                .size(10.5),
                        );
                        ui.label(
                            RichText::new(format!("\u{2212}{}", a.removed))
                                .monospace()
                                .color(accents.error)
                                .size(10.5),
                        );
                    }
                    if let Some(data) = spark {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            widgets::sparkline(ui, data, vec2(48.0, 14.0), st);
                        });
                    }
                });
                ui.horizontal(|ui| {
                    let open = ui
                        .add_enabled(mine, egui::Button::new(RichText::new("Open").small()))
                        .on_hover_text("Open this workspace's chat")
                        .on_disabled_hover_text(
                            "Read-along for teammates' puppies isn't wired yet",
                        );
                    if open.clicked()
                        && let Some(ws) = sup.iter().find(|w| w.name == a.dir)
                    {
                        actions.push(ShellAction::FocusChat(ws.id));
                    }
                    if ui
                        .button(RichText::new("\u{1f44b} Nudge").small())
                        .on_hover_text("Post a nudge to the room for this puppy")
                        .clicked()
                    {
                        let from = if my_puppy.is_empty() { me } else { my_puppy };
                        let what = if a.file.is_empty() { &a.dir } else { &a.file };
                        client.puppy_msg(
                            from,
                            &a.puppy,
                            false,
                            &format!("\u{1f44b} ping on {what}"),
                        );
                        toasts.push(format!("Nudged {}", a.puppy), accents.accent);
                    }
                });
            })
            .response;
        // Owner-colored left rule (the mock's --owner border).
        let r = resp.rect;
        ui.painter().line_segment(
            [
                r.left_top() + vec2(0.5, 4.0),
                r.left_bottom() + vec2(0.5, -4.0),
            ],
            Stroke::new(2.0, owner),
        );
    });
    ui.add_space(4.0);
}
