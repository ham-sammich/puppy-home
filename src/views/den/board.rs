//! Den board: the Shared-plans strip (plans.md checklists + share/unshare)
//! above the 4-column kanban. All ops are buttons/menus over the relay's
//! typed task messages — drag-and-drop is explicitly deferred.

use eframe::egui::{self, CornerRadius, RichText, Stroke};
use puppy_relay::protocol::{TaskColumn, TaskInfo};

use crate::pack::PackClient;
use crate::supervisor::Supervisor;
use crate::theme::Accents;
use crate::views::widgets::Toasts;

use super::{Conn, member_color};

const COLUMNS: [(TaskColumn, &str); 4] = [
    (TaskColumn::Backlog, "Backlog"),
    (TaskColumn::InProgress, "In progress"),
    (TaskColumn::Review, "Review"),
    (TaskColumn::Done, "Done"),
];

/// Parse plans.md checklist lines: `- [ ] open` / `- [x] done` (also `*`).
/// Non-checklist lines are skipped — the wire carries raw markdown.
pub(super) fn plan_items(md: &str) -> impl Iterator<Item = (bool, &str)> {
    md.lines().filter_map(|l| {
        let t = l.trim_start();
        let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
        if let Some(rest) = t.strip_prefix("[x] ").or_else(|| t.strip_prefix("[X] ")) {
            Some((true, rest))
        } else {
            t.strip_prefix("[ ] ").map(|rest| (false, rest))
        }
    })
}

#[allow(clippy::too_many_arguments)] // a render seam, not an API
pub(super) fn render(
    ui: &mut egui::Ui,
    conn: &mut Conn,
    sup: &Supervisor,
    accents: &Accents,
    toasts: &mut Toasts,
    me: &str,
    my_puppy: &str,
) {
    let Conn {
        client, den, board, ..
    } = conn;
    plans_strip(ui, client, den, sup, accents, toasts, me, my_puppy);
    ui.add_space(10.0);
    kanban(ui, client, den, board, accents);
}

/// One plan-card per shared plans.md, plus the Share/Unshare controls.
#[allow(clippy::too_many_arguments)] // a render seam, not an API
fn plans_strip(
    ui: &mut egui::Ui,
    client: &PackClient,
    den: &crate::pack::DenState,
    sup: &Supervisor,
    accents: &Accents,
    toasts: &mut Toasts,
    me: &str,
    my_puppy: &str,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{1f4c4} Shared plans").strong());
        ui.label(
            RichText::new("puppies that wrote a plans.md")
                .weak()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Share my plans.md: lists every open local workspace that has one
            // (checked only while the menu is open). Multiple → you pick.
            ui.menu_button("Share plans.md \u{25be}", |ui| {
                let mut any = false;
                for w in sup.iter().filter(|w| !w.is_remote()) {
                    let path = w.root.join("plans.md");
                    if !path.exists() {
                        continue;
                    }
                    any = true;
                    if ui.button(&w.name).clicked() {
                        match std::fs::read_to_string(&path) {
                            Ok(md) => {
                                let puppy = if my_puppy.is_empty() { me } else { my_puppy };
                                client.plan_share(puppy, &md);
                                toasts.push(
                                    format!("Shared {}'s plans.md with the den", w.name),
                                    accents.accent,
                                );
                            }
                            Err(e) => {
                                toasts.push(format!("Couldn't read plans.md: {e}"), accents.error)
                            }
                        }
                        ui.close();
                    }
                }
                if !any {
                    ui.weak("No open workspace has a plans.md");
                }
                if let Some(mine) = den.plans.iter().find(|p| p.user == me) {
                    ui.separator();
                    if ui.button("Unshare mine").clicked() {
                        client.plan_unshare(&mine.puppy);
                        toasts.push("Plan withdrawn", accents.paused);
                        ui.close();
                    }
                }
            });
        });
    });
    ui.add_space(4.0);

    if den.plans.is_empty() {
        ui.label(RichText::new("No plans shared yet.").weak().small());
        return;
    }
    let weak = ui.visuals().weak_text_color();
    ui.horizontal_wrapped(|ui| {
        for p in &den.plans {
            let owner = member_color(&den.members, &p.user).unwrap_or(accents.accent);
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(Stroke::new(1.0, owner.linear_multiply(0.45)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(9.0)
                .show(ui, |ui| {
                    ui.set_width(250.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("\u{1f436} {}", p.puppy))
                                .color(owner)
                                .strong()
                                .small(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("plans.md").monospace().weak().size(10.0));
                        });
                    });
                    let mut shown = 0usize;
                    let mut total = 0usize;
                    for (done, text) in plan_items(&p.markdown) {
                        total += 1;
                        if shown >= 8 {
                            continue;
                        }
                        shown += 1;
                        if done {
                            ui.label(
                                RichText::new(format!("\u{2713} {text}"))
                                    .strikethrough()
                                    .color(weak)
                                    .size(11.0),
                            );
                        } else {
                            ui.label(RichText::new(format!("\u{25cb} {text}")).size(11.0));
                        }
                    }
                    if total > shown {
                        ui.label(
                            RichText::new(format!("+{} more", total - shown))
                                .weak()
                                .small(),
                        );
                    }
                    if total == 0 {
                        ui.label(RichText::new("(no checklist items)").weak().small());
                    }
                });
        }
    });
}

/// The 4-column kanban. Cards carry a title, an owner puppy chip (or
/// "unassigned"), and a plan tag; the ⋯ menu does move/assign/retitle/delete.
fn kanban(
    ui: &mut egui::Ui,
    client: &PackClient,
    den: &crate::pack::DenState,
    board: &mut super::BoardUi,
    accents: &Accents,
) {
    let weak = ui.visuals().weak_text_color();
    ui.columns(4, |cols| {
        for (i, (col, label)) in COLUMNS.iter().enumerate() {
            let ui = &mut cols[i];
            let count = den.tasks.iter().filter(|t| t.column == *col).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(*label).strong().small());
                ui.label(RichText::new(count.to_string()).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{ff0b}")
                        .on_hover_text("Add a card here")
                        .clicked()
                    {
                        board.add = Some((*col, String::new()));
                    }
                });
            });
            // Inline add-card input for this column.
            if let Some((add_col, draft)) = board.add.as_mut()
                && add_col == col
            {
                let mut submit = false;
                let mut cancel = false;
                let field = ui.add(
                    egui::TextEdit::singleline(draft)
                        .desired_width(f32::INFINITY)
                        .hint_text("Card title\u{2026}"),
                );
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                if submit {
                    let title = draft.trim().to_string();
                    if !title.is_empty() {
                        client.task_create(&title, *col, "", false);
                    }
                    cancel = true;
                }
                if cancel {
                    board.add = None;
                }
            }
            for t in den.tasks.iter().filter(|t| t.column == *col) {
                task_card(ui, t, client, den, board, accents, weak);
            }
        }
    });
}

fn task_card(
    ui: &mut egui::Ui,
    t: &TaskInfo,
    client: &PackClient,
    den: &crate::pack::DenState,
    board: &mut super::BoardUi,
    accents: &Accents,
    weak: egui::Color32,
) {
    let owner = (!t.owner.is_empty())
        .then(|| member_color(&den.members, &t.owner))
        .flatten();
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(
            1.0,
            owner
                .map(|c| c.linear_multiply(0.45))
                .unwrap_or(ui.visuals().widgets.noninteractive.bg_stroke.color),
        ))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            // Title (or the inline retitle editor).
            let retitling = matches!(board.retitle, Some((id, _)) if id == t.id);
            if retitling {
                let (_, draft) = board.retitle.as_mut().expect("checked above");
                let mut done = false;
                let field = ui.add(egui::TextEdit::singleline(draft).desired_width(f32::INFINITY));
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let title = draft.trim().to_string();
                    if !title.is_empty() {
                        client.task_retitle(t.id, &title);
                    }
                    done = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = true;
                }
                if done {
                    board.retitle = None;
                }
            } else {
                ui.label(RichText::new(&t.title).size(12.0));
            }
            ui.horizontal(|ui| {
                match (&t.owner, owner) {
                    (o, Some(color)) if !o.is_empty() => {
                        let puppy = den
                            .members
                            .iter()
                            .find(|m| &m.user == o)
                            .map(|m| m.puppy.as_str())
                            .filter(|p| !p.is_empty())
                            .unwrap_or(o.as_str());
                        ui.label(
                            RichText::new(format!("\u{1f436} {puppy}"))
                                .color(color)
                                .size(10.5),
                        );
                    }
                    _ => {
                        ui.label(RichText::new("unassigned").color(weak).size(10.5));
                    }
                }
                if t.plan {
                    ui.label(
                        RichText::new("\u{1f4c4} plan")
                            .color(accents.accent)
                            .size(10.0),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(RichText::new("\u{22ef}").small(), |ui| {
                        ui.menu_button("Move to", |ui| {
                            for (col, label) in COLUMNS {
                                if col != t.column && ui.button(label).clicked() {
                                    client.task_move(t.id, col);
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Assign to", |ui| {
                            for m in &den.members {
                                if m.user != t.owner && ui.button(&m.user).clicked() {
                                    client.task_assign(t.id, &m.user);
                                    ui.close();
                                }
                            }
                            if !t.owner.is_empty() && ui.button("Unassign").clicked() {
                                client.task_assign(t.id, "");
                                ui.close();
                            }
                        });
                        if ui.button("Retitle").clicked() {
                            board.retitle = Some((t.id, t.title.clone()));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Delete").clicked() {
                            client.task_delete(t.id);
                            ui.close();
                        }
                    });
                });
            });
        });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::plan_items;

    #[test]
    fn plan_items_parses_checklists() {
        let md = "# Plan\n\n- [x] wire form\n- [ ] spinner\n* [X] caps done\n* [ ] star open\nplain prose\n- not a box\n";
        let items: Vec<(bool, &str)> = plan_items(md).collect();
        assert_eq!(
            items,
            vec![
                (true, "wire form"),
                (false, "spinner"),
                (true, "caps done"),
                (false, "star open"),
            ]
        );
    }

    #[test]
    fn plan_items_empty_for_prose_only() {
        assert_eq!(plan_items("just words\nno boxes").count(), 0);
    }
}
