//! The Den's disconnected state: a centered join card (relay / room code /
//! name) in the design language, plus the connect handshake.

use std::collections::HashMap;

use eframe::egui::{self, FontFamily, RichText};
use puppy_relay::protocol::Presence;

use crate::fonts::FAMILY_GROTESK_BOLD;
use crate::pack::{DEN_LABEL, DenState, PackClient};

use super::{BoardUi, Conn, DenView, WorkView};

/// The disconnected state: a centered join card in the design language.
pub(super) fn render_join_form(ui: &mut egui::Ui, view: &mut DenView) {
    ui.add_space((ui.available_height() * 0.5 - 160.0).max(12.0));
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(format!("\u{1f43e} Puppy {DEN_LABEL}"))
                .family(FontFamily::Name(FAMILY_GROTESK_BOLD.into()))
                .size(22.0),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "A shared room: teammates connect and everyone's puppy works the \
                 project together. The room code is the shared secret."
            ))
            .weak(),
        );
        ui.add_space(10.0);
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(egui::CornerRadius::same(13))
            .inner_margin(16.0)
            .show(ui, |ui| {
                egui::Grid::new("den-join-grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Relay").weak());
                        ui.add(
                            egui::TextEdit::singleline(&mut view.relay)
                                .desired_width(240.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("host[:port]"),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Room code").weak());
                        ui.add(
                            egui::TextEdit::singleline(&mut view.room)
                                .desired_width(240.0)
                                .font(egui::TextStyle::Monospace)
                                .hint_text("swift-otter-42"),
                        );
                        ui.end_row();
                        ui.label(RichText::new("Your name").weak());
                        ui.add(egui::TextEdit::singleline(&mut view.user).desired_width(240.0));
                        ui.end_row();
                    });
                if let Some(err) = &view.error {
                    ui.add_space(4.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                ui.add_space(8.0);
                let ready = !view.relay.trim().is_empty()
                    && !view.room.trim().is_empty()
                    && !view.user.trim().is_empty();
                let join = egui::Button::new(
                    RichText::new(format!("Join {DEN_LABEL}")).color(ui.visuals().extreme_bg_color),
                )
                .fill(ui.visuals().hyperlink_color)
                .corner_radius(egui::CornerRadius::same(9));
                if ui.add_enabled(ready, join).clicked() {
                    connect(view, ui.ctx());
                }
            });
        ui.add_space(6.0);
        ui.label(
            RichText::new("Run a relay anywhere reachable:  puppy-relay [port]")
                .weak()
                .small(),
        );
    });
}

pub(super) fn connect(view: &mut DenView, ctx: &egui::Context) {
    match PackClient::connect(
        view.relay.trim(),
        view.room.trim(),
        view.user.trim(),
        view.puppy.trim(),
        crate::waker::egui_waker(ctx),
    ) {
        Ok((client, rx)) => {
            view.error = None;
            view.conn = Some(Conn {
                client,
                rx,
                addr: view.relay.trim().to_string(),
                room: view.room.trim().to_string(),
                activity: HashMap::new(),
                claims: Vec::new(),
                den: DenState::default(),
                input: String::new(),
                work: WorkView::Roster,
                show_all_feed: false,
                sparks: HashMap::new(),
                spark_seen: HashMap::new(),
                presence_sent: Presence::Active,
                board: BoardUi::default(),
            });
        }
        Err(e) => view.error = Some(e),
    }
}
