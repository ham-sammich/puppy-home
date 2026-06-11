//! Helpers shared by the manager views (MCP, Skills).
//!
//! These tabs all follow the same load-bearing invariant: global Code Puppy
//! data flows through a *workspace's* sidecar channel, so each manager picks
//! the first ready workspace and shows a hint when none is connected.

use eframe::egui;

use crate::supervisor::Supervisor;
use crate::workspace::Workspace;

/// Mirror Code Puppy's registry rule: alphanumeric plus `-`/`_`, non-empty.
pub fn validate_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a name is required".into());
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name must be alphanumeric (hyphens and underscores allowed)".into());
    }
    Ok(())
}

/// Pick the workspace that serves global Code Puppy data: the first ready one.
pub fn serving_workspace(sup: &Supervisor) -> Option<&Workspace> {
    sup.iter().find(|w| w.is_ready())
}

/// Centered "no workspace connected" hint; `what` names the tab's data.
pub fn no_workspace_hint(ui: &mut egui::Ui, sup: &Supervisor, what: &str) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No Code Puppy connected")
                    .heading()
                    .weak(),
            );
            ui.add_space(4.0);
            if sup.is_empty() {
                ui.weak(format!(
                    "Open a folder to start a workspace - {what} is read through its sidecar."
                ));
            } else {
                ui.weak("Waiting for a workspace's Code Puppy to become ready...");
            }
        });
    });
}

/// Form vs. raw-paste mode for a create/edit wizard. The form is the guided
/// step-by-step builder; paste lets you drop in a whole config and validate it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Form,
    Paste,
}

/// The "[Form] [Paste]" segmented toggle. Returns the mode the user clicked
/// (unchanged if no click) so the caller can seed/parse on the transition.
pub fn mode_toggle(ui: &mut egui::Ui, mode: EditMode) -> EditMode {
    let mut next = mode;
    ui.horizontal(|ui| {
        if ui
            .selectable_label(mode == EditMode::Form, "Form")
            .on_hover_text("Guided step-by-step builder")
            .clicked()
        {
            next = EditMode::Form;
        }
        if ui
            .selectable_label(mode == EditMode::Paste, "Paste")
            .on_hover_text("Paste a whole config and validate it")
            .clicked()
        {
            next = EditMode::Paste;
        }
    });
    next
}

/// A reusable monospace "paste raw config" editor in a bounded scroll area.
/// Each manager seeds it from / parses it with its own format.
pub fn paste_editor(ui: &mut egui::Ui, id: &str, text: &mut String) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, true])
        .max_height(340.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(15)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
        });
}

/// A small animated on/off switch (the canonical egui toggle widget).
pub fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = ui.spacing().interact_size.y * egui::vec2(2.0, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        ui.painter().rect_filled(rect, radius, visuals.bg_fill);
        let circle_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(circle_x, rect.center().y);
        ui.painter().circle(
            center,
            0.75 * radius,
            visuals.fg_stroke.color,
            visuals.fg_stroke,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(validate_name("my-server_2").is_ok());
        assert!(validate_name(" padded ").is_ok()); // trimmed before checking
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("bad/slash").is_err());
        assert!(validate_name("dot.dot").is_err());
    }
}
