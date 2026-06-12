//! Den board: the shared-plans strip (checklist-parsed plans.md cards with
//! Share/Unshare) above the 4-column kanban (add per column, per-card menu
//! with Move/Assign/Retitle/Delete via typed relay ops — no drag-drop).

use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px,
};

use puppy_relay::protocol::{TaskColumn, TaskInfo};

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, Tokens};

use super::{DenAction, DenArgs, DenPop, TaskTarget, member_color, owner_chip};

pub const COLUMNS: [(TaskColumn, &str); 4] = [
    (TaskColumn::Backlog, "Backlog"),
    (TaskColumn::InProgress, "In progress"),
    (TaskColumn::Review, "Review"),
    (TaskColumn::Done, "Done"),
];

pub fn board_panel(args: &DenArgs) -> AnyElement {
    div()
        .id("den-board-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2p5()
        .child(plans_strip(args))
        .child(kanban(args))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Shared plans
// ---------------------------------------------------------------------------

/// One checklist row parsed from plans.md ("- [ ]" / "- [x]").
fn checklist(md: &str) -> Vec<(bool, String)> {
    md.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            l.strip_prefix("- [x] ")
                .or_else(|| l.strip_prefix("- [X] "))
                .map(|rest| (true, rest.to_string()))
                .or_else(|| {
                    l.strip_prefix("- [ ] ")
                        .map(|rest| (false, rest.to_string()))
                })
        })
        .collect()
}

fn plans_strip(args: &DenArgs) -> AnyElement {
    let t = args.t;
    let den = args.den;
    let mine_shared = den.state.plans.iter().any(|p| p.user == den.user);

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.weak)
                        .child("SHARED PLANS"),
                )
                .child(div().flex_1())
                .child(share_button(args, mine_shared)),
        )
        .child(if den.state.plans.is_empty() {
            div()
                .text_size(px(11.5))
                .text_color(t.dim)
                .child(
                    "No plans shared yet \u{2014} share your plans.md so the den can see the path.",
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(den.state.plans.iter().map(|p| {
                    let color = crate::gpui_ui::tokens::hex(member_color(den, &p.user));
                    let items = checklist(&p.markdown);
                    let done = items.iter().filter(|(d, _)| *d).count();
                    div()
                        .w(px(260.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2p5()
                        .rounded(px(11.))
                        .bg(t.card)
                        .border_1()
                        .border_color(alpha(color, 0.5))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(color)
                                        .child(format!("\u{1f4c4} {}", p.puppy)),
                                )
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(t.weak)
                                        .child(format!("{done}/{} done", items.len())),
                                ),
                        )
                        .children(items.into_iter().take(8).map(|(done, text)| {
                            div()
                                .flex()
                                .items_start()
                                .gap_1p5()
                                .text_size(px(11.))
                                .child(
                                    div()
                                        .text_color(if done { t.run } else { t.dim })
                                        .child(if done { "\u{2611}" } else { "\u{2610}" }),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_color(if done { t.dim } else { t.text })
                                        .when(done, |d| d.line_through())
                                        .child(text),
                                )
                        }))
                }))
                .into_any_element()
        })
        .into_any_element()
}

/// Share / Unshare control + the workspace-plans picker popover.
fn share_button(args: &DenArgs, mine_shared: bool) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    if mine_shared {
        return widgets::btn(&t, "Unshare my plan")
            .id("den-unshare")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Den(DenAction::PlanUnshare), cx)
                });
            })
            .into_any_element();
    }
    let open = matches!(args.pop, Some(DenPop::SharePlan));
    let picker = open.then(|| {
        let panel = div()
            .occlude()
            .absolute()
            .top(px(26.))
            .right_0()
            .min_w(px(240.))
            .flex()
            .flex_col()
            .gap_0p5()
            .p_1()
            .rounded(px(10.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            .on_mouse_down_out({
                let root = args.root.clone();
                move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(DenAction::ClosePop), cx)
                    });
                }
            })
            .children(if args.sharable_plans.is_empty() {
                vec![
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(t.weak)
                        .child("No plans.md in any open workspace.")
                        .into_any_element(),
                ]
            } else {
                args.sharable_plans
                    .iter()
                    .enumerate()
                    .map(|(i, (dir, path))| {
                        let root = args.root.clone();
                        let path = path.clone();
                        div()
                            .id(("den-share-opt", i as u64))
                            .px_2()
                            .py_1()
                            .rounded(px(7.))
                            .font_family("JetBrains Mono")
                            .text_size(px(11.5))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|d| d.bg(t.well))
                            .child(format!("{dir}/plans.md"))
                            .on_click(move |_, _, cx| {
                                root.update(cx, |r, cx| {
                                    r.dispatch(
                                        DashAction::Den(DenAction::PlanShare(path.clone())),
                                        cx,
                                    )
                                });
                            })
                            .into_any_element()
                    })
                    .collect()
            });
        gpui::deferred(panel).with_priority(100)
    });
    div()
        .relative()
        .child(widgets::btn(&t, "Share to den").id("den-share").on_click({
            let root = args.root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Den(DenAction::TogglePop(DenPop::SharePlan)), cx)
                });
            }
        }))
        .children(picker)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Kanban
// ---------------------------------------------------------------------------

fn kanban(args: &DenArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .gap_2()
        .items_start()
        .children(COLUMNS.iter().map(|(col, label)| {
            let cards: Vec<&TaskInfo> = args
                .den
                .state
                .tasks
                .iter()
                .filter(|task| task.column == *col)
                .collect();
            let adding = args.task_target == Some(TaskTarget::Add(*col));
            div()
                .flex_1()
                .min_w(px(150.))
                .flex()
                .flex_col()
                .gap_1p5()
                .p_2()
                .rounded(px(11.))
                .bg(t.panel)
                .border_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.weak)
                                .child(format!("{label} ({})", cards.len())),
                        )
                        .child(div().flex_1())
                        .child(add_button(args, *col, adding)),
                )
                .children(cards.into_iter().map(|task| card(args, task)))
                .children(
                    (adding && args.task_input.is_some())
                        .then(|| task_input_row(&t, args.task_input.unwrap())),
                )
        }))
        .into_any_element()
}

fn add_button(args: &DenArgs, col: TaskColumn, on: bool) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    div()
        .id(("den-add", col as u64))
        .px_1p5()
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(if on { t.accent } else { t.weak })
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .child("\u{ff0b}")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| {
                r.dispatch(DashAction::Den(DenAction::TaskAdd(col)), cx)
            });
        })
        .into_any_element()
}

fn task_input_row(
    t: &Tokens,
    input: &gpui::Entity<crate::gpui_ui::input::ChatInput>,
) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(alpha(t.accent, 0.5))
        .font_family("JetBrains Mono")
        .text_size(px(11.5))
        .child(input.clone())
        .into_any_element()
}

fn card(args: &DenArgs, task: &TaskInfo) -> AnyElement {
    let t = args.t;
    let den = args.den;
    let retitling = args.task_target == Some(TaskTarget::Retitle(task.id));
    let menu_open = matches!(args.pop, Some(DenPop::TaskMenu(id)) if *id == task.id);

    let menu = menu_open.then(|| {
        let mk = |label: String, idx: u64, action: DenAction| {
            let root = args.root.clone();
            div()
                .id(("den-task-act", idx))
                .px_2()
                .py_0p5()
                .rounded(px(6.))
                .text_size(px(11.))
                .text_color(t.text)
                .cursor_pointer()
                .hover(|d| d.bg(t.well))
                .child(label)
                .on_click(move |_, _, cx| {
                    let a = action.clone();
                    root.update(cx, |r, cx| r.dispatch(DashAction::Den(a), cx));
                })
        };
        let mut items: Vec<AnyElement> = Vec::new();
        for (col, label) in COLUMNS.iter().filter(|(c, _)| *c != task.column) {
            items.push(
                mk(
                    format!("Move \u{2192} {label}"),
                    *col as u64,
                    DenAction::TaskMove(task.id, *col),
                )
                .into_any_element(),
            );
        }
        if task.owner == den.user {
            items.push(
                mk("Unassign".into(), 10, DenAction::TaskUnassign(task.id)).into_any_element(),
            );
        } else {
            items.push(
                mk("Assign to me".into(), 10, DenAction::TaskAssignMe(task.id)).into_any_element(),
            );
        }
        items.push(mk("Retitle".into(), 11, DenAction::TaskRetitle(task.id)).into_any_element());
        items.push(mk("Delete".into(), 12, DenAction::TaskDelete(task.id)).into_any_element());
        let panel = div()
            .occlude()
            .absolute()
            .top(px(20.))
            .right_0()
            .min_w(px(150.))
            .flex()
            .flex_col()
            .gap_0p5()
            .p_1()
            .rounded(px(9.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            .on_mouse_down_out({
                let root = args.root.clone();
                move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(DenAction::ClosePop), cx)
                    });
                }
            })
            .children(items);
        gpui::deferred(panel).with_priority(100)
    });

    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded(px(9.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex()
                .items_start()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(t.text)
                        .child(task.title.clone()),
                )
                .child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id(("den-task-menu", task.id))
                                .px_1()
                                .rounded(px(5.))
                                .text_color(t.weak)
                                .cursor_pointer()
                                .hover(|d| d.bg(t.well))
                                .child("\u{22ef}")
                                .on_click({
                                    let root = args.root.clone();
                                    let id = task.id;
                                    move |_, _, cx| {
                                        root.update(cx, |r, cx| {
                                            r.dispatch(
                                                DashAction::Den(DenAction::TogglePop(
                                                    DenPop::TaskMenu(id),
                                                )),
                                                cx,
                                            )
                                        });
                                    }
                                }),
                        )
                        .children(menu),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .children(if task.owner.is_empty() {
                    Some(
                        div()
                            .text_size(px(9.5))
                            .text_color(t.dim)
                            .child("unassigned")
                            .into_any_element(),
                    )
                } else {
                    Some(owner_chip(&t, &task.owner, member_color(den, &task.owner)))
                })
                .children(task.plan.then(|| {
                    div()
                        .text_size(px(9.5))
                        .text_color(t.weak)
                        .child("\u{1f4c4} plan")
                })),
        )
        .children(
            (retitling && args.task_input.is_some())
                .then(|| task_input_row(&t, args.task_input.unwrap())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_parses_done_and_open() {
        let md = "# plan\n- [x] ship dashboard\n- [ ] ship chat\nnot a task\n- [X] caps too";
        let items = checklist(md);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], (true, "ship dashboard".into()));
        assert_eq!(items[1], (false, "ship chat".into()));
        assert_eq!(items[2], (true, "caps too".into()));
    }
}
