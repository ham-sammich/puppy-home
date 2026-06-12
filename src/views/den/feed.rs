//! The Den's coordination feed (right sidebar): humans, puppy-to-puppy
//! messages (owner-colored, review badges), and system events, with a
//! composer at the bottom. Stick-to-bottom + the transcript's tail-cap
//! pattern keep long sessions cheap.

use eframe::egui::{self, CornerRadius, RichText, Stroke, vec2};
use puppy_relay::protocol::{FeedEntry, FeedKind};

use crate::pack::DEN_LABEL;
use crate::theme::Accents;

use super::{Conn, member_color};

/// How many feed entries render by default (the ring caps at 500; "Show
/// older" opts into the rest — same pattern as the chat transcript).
const FEED_RENDER_TAIL: usize = 150;

pub(super) fn render(ui: &mut egui::Ui, conn: &mut Conn, accents: &Accents, me: &str) {
    let Conn {
        client,
        den,
        input,
        show_all_feed,
        ..
    } = conn;

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{DEN_LABEL} feed")).strong());
        ui.label(
            RichText::new("people + puppies coordinating")
                .weak()
                .small(),
        );
    });
    ui.separator();

    // Composer pinned to the bottom of the sidebar.
    egui::Panel::bottom(egui::Id::new("den-feed-composer")).show_inside(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(input)
                    .desired_width((ui.available_width() - 52.0).max(80.0))
                    .hint_text(format!("Message the {}\u{2026}", DEN_LABEL.to_lowercase())),
            );
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Send").clicked() || enter) && !input.trim().is_empty() {
                client.chat(input.trim());
                input.clear();
                field.request_focus();
            }
        });
        ui.add_space(4.0);
    });

    // The feed list: relay-ordered, bounded ring; render the recent tail.
    let total = den.feed.len();
    let start = if *show_all_feed {
        0
    } else {
        total.saturating_sub(FEED_RENDER_TAIL)
    };
    let weak = ui.visuals().weak_text_color();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .id_salt("den-feed-scroll")
        .show(ui, |ui| {
            if start > 0 {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{start} older hidden"))
                            .weak()
                            .small(),
                    );
                    if ui.small_button("Show older").clicked() {
                        *show_all_feed = true;
                    }
                });
            }
            for e in den.feed.iter().skip(start) {
                entry(ui, e, den, accents, weak, me);
            }
        });
}

fn entry(
    ui: &mut egui::Ui,
    e: &FeedEntry,
    den: &crate::pack::DenState,
    accents: &Accents,
    weak: egui::Color32,
    me: &str,
) {
    match e.kind {
        FeedKind::System => {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(&e.text).weak().small().italics());
            });
            ui.add_space(4.0);
        }
        FeedKind::Human => {
            let color = member_color(&den.members, &e.user).unwrap_or(weak);
            let name = if e.user == me {
                format!("{} (you)", e.user)
            } else {
                e.user.clone()
            };
            ui.label(RichText::new(name).color(color).strong().small());
            ui.label(RichText::new(&e.text).size(12.5));
            ui.add_space(6.0);
        }
        FeedKind::Puppy => {
            let color = member_color(&den.members, &e.user).unwrap_or(accents.accent);
            let resp = ui
                .scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(7.0);
                        ui.label(
                            RichText::new(format!("\u{1f436} {}", e.puppy))
                                .color(color)
                                .strong()
                                .small(),
                        );
                        if !e.user.is_empty() {
                            ui.label(
                                RichText::new(format!("{}'s puppy", e.user))
                                    .weak()
                                    .size(10.0),
                            );
                        }
                        if !e.to_puppy.is_empty() {
                            ui.label(
                                RichText::new(format!("\u{2192} {}", e.to_puppy))
                                    .weak()
                                    .small(),
                            );
                        }
                        if e.review {
                            egui::Frame::new()
                                .stroke(Stroke::new(1.0, accents.wait.linear_multiply(0.7)))
                                .corner_radius(CornerRadius::same(255))
                                .inner_margin(egui::Margin::symmetric(5, 1))
                                .show(ui, |ui| {
                                    ui.label(RichText::new("review").color(accents.wait).size(9.5));
                                });
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(7.0);
                        ui.label(RichText::new(&e.text).size(12.0));
                    });
                })
                .response;
            // Owner-colored left rule.
            let r = resp.rect;
            ui.painter().line_segment(
                [
                    r.left_top() + vec2(1.0, 2.0),
                    r.left_bottom() + vec2(1.0, -2.0),
                ],
                Stroke::new(2.0, color),
            );
            ui.add_space(6.0);
        }
    }
}
