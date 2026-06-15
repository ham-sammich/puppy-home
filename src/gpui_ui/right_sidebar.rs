//! The right collapsible sidebar: a mirror of the left Explorer rail, docked
//! to the right edge of the chat layout. It stacks three individually-
//! collapsible sections — KENNEL (memory browser), GOALS (a stub HUD for a
//! later task), and JUDGES (the goal-mode verifier roster) — and, like the
//! Explorer, shrinks to a slim toggle rail when hidden.
//!
//! State lives on `RootView` (the kennel DB + judges.json are global, so the
//! sidebar's open/scope state is global too); the *data* is folded into the
//! focused workspace by the wire decode and read back here for render. The
//! drain loop drives `sidebar_upkeep` to fetch on a gentle cadence.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers::{MgrAction, MgrKind};
use crate::gpui_ui::widgets::{self, alpha, fmt_ago};
use crate::gpui_ui::{DashAction, RootView, Screen, Tokens};
use crate::workspace::{Workspace, WorkspaceId};

/// Re-poll cadence for the kennel/judges feeds while the sidebar is open
/// (memory + judges change rarely; the same SLOW_REFRESH spirit as managers).
const SIDEBAR_REFRESH: Duration = Duration::from_secs(8);
/// Bounded render tail for the drawer list (like the transcript cap).
const DRAWER_TAIL: usize = 80;

// ---------------------------------------------------------------------------
// RootView wiring: focus resolution, scope, polling
// ---------------------------------------------------------------------------

impl RootView {
    /// The workspace the sidebar reads through: the open chat's workspace, or
    /// the first ready one (the managers' `serving_ws` invariant) as fallback.
    pub(crate) fn focused_ws(&self) -> Option<&Workspace> {
        if let Screen::Chat(id) = self.screen
            && let Some(ws) = self.supervisor.get(id)
        {
            return Some(ws);
        }
        self.serving_ws()
    }

    fn focused_ws_id(&self) -> Option<WorkspaceId> {
        self.focused_ws().map(|w| w.id)
    }

    /// Run a closure with the focused workspace's backend, if it has one.
    pub(crate) fn with_focused_backend(&self, f: impl FnOnce(&crate::backend::CodePuppy)) {
        if let Some(ws) = self.focused_ws()
            && let Some(backend) = &ws.backend
        {
            f(backend);
        }
    }

    /// The recall scope for kennel queries: the selected wings (empty = all).
    pub(crate) fn kennel_selected_wings(&self) -> Vec<String> {
        let mut v: Vec<String> = self.kennel_wings_sel.iter().cloned().collect();
        v.sort();
        v
    }

    /// Re-fetch the kennel view honoring the current search box (search when
    /// non-empty, recent otherwise) over the selected wings.
    pub(crate) fn refresh_kennel(&mut self, cx: &mut gpui::Context<Self>) {
        let q = self
            .kennel_search_input
            .as_ref()
            .map(|i| i.read(cx).text().to_string())
            .unwrap_or_default();
        let wings = self.kennel_selected_wings();
        if q.trim().is_empty() {
            self.with_focused_backend(|b| b.kennel_recent(&wings, 60));
        } else {
            self.with_focused_backend(|b| b.kennel_search(&q, &wings, 60));
        }
    }

    /// The focused workspace's default recall scope: its repo wing, its agent
    /// wing, and every user wing — intersected with the wings that exist.
    fn default_scope(&self, available: &[crate::backend::KennelWing]) -> HashSet<String> {
        let Some(ws) = self.focused_ws() else {
            return HashSet::new();
        };
        let repo_a = format!("repo:{}", ws.root.display());
        let repo_b = format!("repo:{}", ws.cwd);
        let agent = format!("agent:{}", ws.agent);
        available
            .iter()
            .map(|w| w.name.clone())
            .filter(|n| *n == repo_a || *n == repo_b || *n == agent || n.starts_with("user:"))
            .collect()
    }

    /// Lazily create the sidebar inputs (entities can't spawn in render).
    fn ensure_sidebar_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        if self.kennel_search_input.is_none() {
            let entity = cx.new(|cx| ChatInput::new("\u{1f50e} search the kennel\u{2026}", cx));
            let sub = cx.subscribe(&entity, |this, _, ev: &crate::gpui_ui::InputEvent, cx| {
                if matches!(ev, crate::gpui_ui::InputEvent::Submitted) {
                    this.dispatch(DashAction::KennelSearch, cx);
                }
                cx.notify();
            });
            self.kennel_search_input = Some(entity);
            self.chat_subs.push(sub);
        }
        if self.goal_input.is_none() {
            let entity = cx.new(|cx| ChatInput::new("describe the goal\u{2026}", cx));
            let sub = cx.subscribe(&entity, |this, _, ev: &crate::gpui_ui::InputEvent, cx| {
                if matches!(ev, crate::gpui_ui::InputEvent::Submitted) {
                    this.dispatch(DashAction::GoalStart, cx);
                }
                cx.notify();
            });
            self.goal_input = Some(entity);
            self.chat_subs.push(sub);
        }
    }

    /// Drain-tick upkeep: when the right sidebar is open, keep its inputs alive
    /// and poll the kennel/judges feeds on a gentle cadence. Seeds the recall
    /// scope to the focused workspace's repo/agent/user wings once wings load.
    pub(crate) fn sidebar_upkeep(&mut self, cx: &mut gpui::Context<Self>) {
        // Judges feed also powers the Judges manager overlay, so poll it when
        // either surface is visible.
        let judges_visible =
            (!self.right_closed && self.judges_open) || self.manager_open == Some(MgrKind::Judges);
        if self.right_closed && self.manager_open != Some(MgrKind::Judges) {
            return;
        }
        self.ensure_sidebar_inputs(cx);

        let Some(ws_id) = self.focused_ws_id() else {
            return;
        };
        // Seed the default scope once the wings have loaded.
        if !self.kennel_scope_seeded
            && let Some(wings) = self.focused_ws().map(|w| w.kennel_wings.clone())
            && let Some(wings) = wings
        {
            self.kennel_wings_sel = self.default_scope(&wings);
            self.kennel_scope_seeded = true;
        }
        // A generation bump (fresh wire data) clears the staleness gate so the
        // view reflects it promptly.
        let generation = self.focused_ws().map(|w| w.kennel_generation).unwrap_or(0);
        if self.kennel_seen != Some((ws_id, generation)) {
            self.kennel_seen = Some((ws_id, generation));
        }
        let stale = self
            .kennel_last_req
            .is_none_or(|at| at.elapsed() >= SIDEBAR_REFRESH);
        if !stale {
            return;
        }
        self.kennel_last_req = Some(Instant::now());
        let wings = self.kennel_selected_wings();
        let q = self
            .kennel_search_input
            .as_ref()
            .map(|i| i.read(cx).text().to_string())
            .unwrap_or_default();
        self.with_focused_backend(|b| {
            if !self.right_closed && self.kennel_open {
                b.kennel_stats();
                b.kennel_list_wings();
                if q.trim().is_empty() {
                    b.kennel_recent(&wings, 60);
                } else {
                    b.kennel_search(&q, &wings, 60);
                }
            }
            if judges_visible {
                b.list_judges();
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Everything the right sidebar needs to draw (borrowed from RootView).
pub struct RightArgs<'a> {
    pub t: Tokens,
    pub root: Entity<RootView>,
    pub ws: &'a Workspace,
    pub closed: bool,
    pub kennel_open: bool,
    pub goals_open: bool,
    pub judges_open: bool,
    pub wings_sel: &'a HashSet<String>,
    pub expanded: Option<i64>,
    pub search_input: Option<&'a Entity<ChatInput>>,
    pub goal_input: Option<&'a Entity<ChatInput>>,
    pub goal_notes_open: &'a HashSet<String>,
    pub reduce_motion: bool,
}

/// The right sidebar panel when open, or a slim rail with the toggle when not.
pub fn right_sidebar(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    let toggle = div()
        .id("right-toggle")
        .px_1p5()
        .py_0p5()
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(if !args.closed { t.accent } else { t.weak })
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .tooltip(widgets::text_tip(
            "Kennel \u{b7} Goals \u{b7} Judges sidebar".into(),
        ))
        .child("\u{25a5}")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::ToggleRightSidebar, cx));
        });

    if args.closed {
        return div().pl_2().child(toggle).into_any_element();
    }

    div()
        .w(px(268.))
        .flex_none()
        .ml_3()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(12.))
        .border_1()
        .border_color(t.line_soft)
        .bg(t.card)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex_1()
                        .text_size(px(10.5))
                        .text_color(t.weak)
                        .child("SIDEBAR"),
                )
                .child(toggle),
        )
        .child(
            div()
                .id("right-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .px_1p5()
                .pb_2()
                .child(kennel_section(args))
                .child(goals_section(args))
                .child(judges_section(args)),
        )
        .into_any_element()
}

/// A collapsible section header (chevron + title + optional right slot).
fn section_header(
    args: &RightArgs,
    which: u8,
    title: &str,
    open: bool,
    right: Option<AnyElement>,
) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    div()
        .id(("right-section", which as u64))
        .flex()
        .items_center()
        .gap_1()
        .px_1()
        .py_0p5()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .child(div().text_size(px(10.)).text_color(t.weak).child(if open {
            "\u{25be}"
        } else {
            "\u{25b8}"
        }))
        .child(
            div()
                .flex_1()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(title.to_string()),
        )
        .children(right)
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| {
                r.dispatch(DashAction::ToggleRightSection(which), cx)
            });
        })
        .into_any_element()
}

// --- KENNEL ----------------------------------------------------------------

fn kennel_section(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let count_label = ws
        .kennel_stats
        .as_ref()
        .map(|s| format!("{} drawers \u{b7} {} wings", s.drawers, s.wings))
        .unwrap_or_else(|| "\u{2026}".to_string());
    let header = section_header(
        args,
        0,
        "KENNEL",
        args.kennel_open,
        Some(
            div()
                .text_size(px(9.5))
                .text_color(t.dim)
                .child(count_label)
                .into_any_element(),
        ),
    );
    let mut col = div().flex().flex_col().gap_1().child(header);
    if !args.kennel_open {
        return col.into_any_element();
    }

    // Wing list (counts; selected wings highlighted). Empty selection = all.
    if let Some(wings) = &ws.kennel_wings {
        let mut list = div().flex().flex_col().gap_0p5();
        for (i, w) in wings.iter().enumerate() {
            let on = args.wings_sel.contains(&w.name);
            let root = args.root.clone();
            let name = w.name.clone();
            list = list.child(
                div()
                    .id(("kennel-wing", i as u64))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py_0p5()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(alpha(t.accent, 0.12)))
                    .hover(|d| d.bg(t.well))
                    .tooltip(widgets::text_tip(w.name.clone()))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(10.5))
                            .text_color(if on { t.accent } else { t.weak })
                            .child(wing_label(&w.name)),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(t.dim)
                            .child(format!("{}", w.count)),
                    )
                    .on_click(move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::KennelWing(name.clone()), cx)
                        });
                    }),
            );
        }
        col = col.child(list);
    }

    // Search box + reset.
    col = col.child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(6.))
                    .bg(t.well)
                    .border_1()
                    .border_color(t.line_soft)
                    .font_family("JetBrains Mono")
                    .text_size(px(10.5))
                    .children(args.search_input.cloned()),
            )
            .child({
                let root = args.root.clone();
                widgets::btn(&t, "recent")
                    .id("kennel-recent")
                    .tooltip(widgets::text_tip("Clear the search; show recent".into()))
                    .on_click(move |_, _, cx| {
                        root.update(cx, |r, cx| r.dispatch(DashAction::KennelRecent, cx));
                    })
            }),
    );

    // Drawer list (bounded tail; click to peek full content).
    match &ws.kennel_drawers {
        None => col = col.child(hint(&t, "loading memory\u{2026}")),
        Some(d) if d.is_empty() => col = col.child(hint(&t, "no drawers in this scope.")),
        Some(drawers) => {
            let start = drawers.len().saturating_sub(DRAWER_TAIL);
            let mut list = div().flex().flex_col().gap_0p5();
            for (i, d) in drawers[start..].iter().enumerate() {
                list = list.child(drawer_row(args, i, d));
            }
            col = col.child(list);
        }
    }
    col.into_any_element()
}

/// One drawer row: role + relative ts + agent badge + content preview, with
/// a click-to-peek full body (mono, scrollable, bounded).
fn drawer_row(args: &RightArgs, i: usize, d: &crate::backend::KennelDrawer) -> AnyElement {
    let t = args.t;
    let open = args.expanded == Some(d.id);
    let root = args.root.clone();
    let id = d.id;
    let rel = rel_ts(&d.ts);
    let role = if d.role.is_empty() { "note" } else { &d.role };

    let mut head = div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.accent)
                .child(role.to_string()),
        )
        .child(div().flex_1())
        .child(div().text_size(px(9.)).text_color(t.dim).child(rel));
    if !d.agent.is_empty() && d.agent != "unknown" {
        head = head.child(
            div()
                .px_1()
                .rounded(px(4.))
                .bg(t.well)
                .text_size(px(8.5))
                .text_color(t.weak)
                .child(d.agent.clone()),
        );
    }

    let preview = first_line(&d.content);
    let body: AnyElement = if open {
        div()
            .id(("kennel-body", id as u64))
            .max_h(px(220.))
            .overflow_y_scroll()
            .mt_0p5()
            .px_1p5()
            .py_1()
            .rounded(px(6.))
            .bg(t.well)
            .font_family("JetBrains Mono")
            .text_size(px(10.))
            .text_color(t.text)
            .child(d.content.clone())
            .into_any_element()
    } else {
        div()
            .overflow_hidden()
            .text_ellipsis()
            .text_size(px(10.))
            .text_color(t.weak)
            .child(preview)
            .into_any_element()
    };

    div()
        .id(("kennel-drawer", i as u64))
        .flex()
        .flex_col()
        .px_1p5()
        .py_1()
        .rounded(px(7.))
        .cursor_pointer()
        .when(open, |d| d.bg(alpha(t.accent, 0.08)))
        .hover(|d| d.bg(t.well))
        .child(head)
        .child(body)
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::KennelExpand(id), cx));
        })
        .into_any_element()
}

// --- GOALS (live HUD) ------------------------------------------------------

fn goals_section(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let g = &args.ws.goal;
    // A compact state pill in the header (idle / running / complete / stopped).
    let (pill_text, pill_color) = goal_pill(g, &t);
    let header = section_header(
        args,
        1,
        "GOALS",
        args.goals_open,
        Some(
            div()
                .px_1()
                .rounded(px(4.))
                .bg(alpha(pill_color, 0.16))
                .text_size(px(8.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pill_color)
                .child(pill_text)
                .into_any_element(),
        ),
    );
    let mut col = div().flex().flex_col().gap_1().child(header);
    if !args.goals_open {
        return col.into_any_element();
    }

    if g.is_active() {
        // Active: prompt + loop N/max + latest remediation + Stop.
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .py_1p5()
                .rounded(px(7.))
                .bg(t.well)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(9.5))
                                .text_color(t.dim)
                                .child(format!("loop {}/{}", g.loop_count.max(1), g.max)),
                        )
                        .child(stop_btn(args)),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(t.text)
                        .child(g.prompt.clone()),
                )
                .children((!g.remediation.is_empty()).then(|| {
                    div()
                        .id("goal-remediation")
                        .max_h(px(120.))
                        .overflow_y_scroll()
                        .px_1p5()
                        .py_1()
                        .rounded(px(6.))
                        .bg(t.card)
                        .font_family("JetBrains Mono")
                        .text_size(px(9.5))
                        .text_color(t.weak)
                        .child(g.remediation.clone())
                })),
        );
    } else {
        // Idle / finished: show the last result, then the start control.
        if let Some(done) = &g.done {
            col = col.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(7.))
                    .bg(t.well)
                    .text_size(px(10.))
                    .text_color(t.weak)
                    .child(format!(
                        "{} after {} loop(s) \u{b7} {}",
                        if done.completed { "completed" } else { "ended" },
                        done.loops,
                        done.reason
                    )),
            );
        }
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(6.))
                        .bg(t.well)
                        .border_1()
                        .border_color(t.line_soft)
                        .text_size(px(10.5))
                        .children(args.goal_input.cloned()),
                )
                .child({
                    let root = args.root.clone();
                    widgets::btn(&t, "Start goal")
                        .id("goal-start")
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| r.dispatch(DashAction::GoalStart, cx));
                        })
                }),
        );
    }
    col.into_any_element()
}

fn stop_btn(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    div()
        .id("goal-stop")
        .px_1p5()
        .py_0p5()
        .rounded(px(5.))
        .border_1()
        .border_color(alpha(t.error, 0.7))
        .text_size(px(9.5))
        .text_color(t.error)
        .cursor_pointer()
        .hover(|d| d.bg(t.well))
        .child("\u{25a0} Stop")
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::GoalStop, cx));
        })
        .into_any_element()
}

/// The GOALS header pill: idle / running / complete / stopped + its color.
fn goal_pill(g: &crate::workspace::goal::GoalRun, t: &Tokens) -> (String, gpui::Rgba) {
    if g.is_active() {
        ("running".into(), t.accent)
    } else if let Some(done) = &g.done {
        if done.completed {
            ("complete".into(), t.run)
        } else {
            ("stopped".into(), t.dim)
        }
    } else {
        ("idle".into(), t.dim)
    }
}

// --- JUDGES ----------------------------------------------------------------

fn judges_section(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let manage = {
        let root = args.root.clone();
        div()
            .id("judges-manage")
            .text_size(px(9.5))
            .text_color(t.accent)
            .cursor_pointer()
            .hover(|d| d.text_color(t.text))
            .child("Manage")
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Mgr(MgrAction::Open(MgrKind::Judges)), cx)
                });
            })
    };
    let header = section_header(
        args,
        2,
        "JUDGES",
        args.judges_open,
        Some(manage.into_any_element()),
    );
    let mut col = div().flex().flex_col().gap_1().child(header);
    if !args.judges_open {
        return col.into_any_element();
    }
    // During (or after) a goal run, the real-time judging view takes over —
    // every enabled judge as a live row resolving pending->running->verdict.
    if ws.goal.has_activity() {
        return col.child(judging_view(args)).into_any_element();
    }
    match &ws.judges {
        None => col = col.child(hint(&t, "loading judges\u{2026}")),
        Some(j) if j.is_empty() => {
            col = col.child(hint(&t, "no judges yet \u{2014} \"Manage\" to add one."))
        }
        Some(judges) => {
            let mut list = div().flex().flex_col().gap_0p5();
            for (i, j) in judges.iter().enumerate() {
                list = list.child(judge_row(args, i, j));
            }
            col = col.child(list);
        }
    }
    col.into_any_element()
}

/// The real-time judging view: the current round's live rows + a bounded
/// scrollback of prior rounds' verdicts.
fn judging_view(args: &RightArgs) -> AnyElement {
    let t = args.t;
    let g = &args.ws.goal;
    let mut col = div().flex().flex_col().gap_1();

    // Current round (live). Header shows the iteration.
    if !g.judges.is_empty() {
        col = col.child(div().text_size(px(9.)).text_color(t.dim).child(format!(
            "iteration {}/{}",
            g.iteration.max(1),
            g.max
        )));
        let mut rows = div().flex().flex_col().gap_0p5();
        for (i, row) in g.judges.iter().enumerate() {
            rows = rows.child(live_judge_row(args, g.iteration, i, row));
        }
        col = col.child(rows);
    }

    // Prior rounds (newest first), bounded by GoalRun's MAX_ROUNDS.
    for round in g.rounds.iter().rev() {
        let verdict = if round.all_complete {
            ("all passed", t.run)
        } else {
            ("incomplete", t.paused)
        };
        col = col.child(
            div()
                .mt_0p5()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(8.5))
                        .text_color(t.dim)
                        .child(format!("round {}", round.iteration)),
                )
                .child(
                    div()
                        .text_size(px(8.5))
                        .text_color(verdict.1)
                        .child(verdict.0),
                ),
        );
        let mut rows = div().flex().flex_col().gap_0p5();
        for (i, row) in round.verdicts.iter().enumerate() {
            rows = rows.child(live_judge_row(args, round.iteration, i, row));
        }
        col = col.child(rows);
    }
    col.into_any_element()
}

/// One live judge row: status dot/label + name + model + expandable notes.
fn live_judge_row(
    args: &RightArgs,
    iteration: u64,
    i: usize,
    row: &crate::workspace::goal::JudgeLive,
) -> AnyElement {
    let t = args.t;
    let color = judge_status_color(row.status, &t);
    let key = format!("{iteration}:{}", row.name);
    let open = args.goal_notes_open.contains(&key);
    let has_notes = !row.notes.trim().is_empty();
    let running = row.status.is_running();

    let head = div()
        .id(("judge-live", (iteration * 97 + i as u64)))
        .flex()
        .items_center()
        .gap_1p5()
        .px_1p5()
        .py_0p5()
        .rounded(px(5.))
        .when(has_notes, |d| d.cursor_pointer().hover(|d| d.bg(t.well)))
        // Running rows pulse (reduce-motion collapses to a static halo).
        .child(widgets::status_dot(
            iteration * 97 + i as u64,
            color,
            running,
            args.reduce_motion,
        ))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(t.text)
                .child(row.name.clone()),
        )
        .children((!row.model.is_empty()).then(|| {
            div()
                .px_1()
                .rounded(px(4.))
                .bg(t.well)
                .font_family("JetBrains Mono")
                .text_size(px(8.))
                .text_color(t.weak)
                .child(short_model(&row.model))
        }))
        .child(
            div()
                .text_size(px(8.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(row.status.label()),
        );
    let head = if has_notes {
        let root = args.root.clone();
        head.on_click(move |_, _, cx| {
            root.update(cx, |r, cx| {
                r.dispatch(DashAction::GoalNotes(key.clone()), cx)
            });
        })
    } else {
        head
    };

    let mut wrap = div().flex().flex_col().child(head);
    if open && has_notes {
        wrap = wrap.child(
            div()
                .id(("judge-notes", iteration * 97 + i as u64))
                .max_h(px(160.))
                .overflow_y_scroll()
                .mx_1p5()
                .mb_0p5()
                .px_1p5()
                .py_1()
                .rounded(px(6.))
                .bg(t.well)
                .font_family("JetBrains Mono")
                .text_size(px(9.5))
                .text_color(t.weak)
                .child(row.notes.clone()),
        );
    }
    wrap.into_any_element()
}

fn judge_status_color(status: crate::workspace::goal::JudgeStatus, t: &Tokens) -> gpui::Rgba {
    use crate::workspace::goal::JudgeStatus;
    match status {
        JudgeStatus::Pending | JudgeStatus::Running => t.think,
        JudgeStatus::Pass => t.run,
        JudgeStatus::Fail => t.error,
        JudgeStatus::Abstain => t.dim,
    }
}

/// Shorten a model id for the narrow pill (keep the last segment).
fn short_model(model: &str) -> String {
    model.rsplit(['/', '-']).next().unwrap_or(model).to_string()
}

fn judge_row(args: &RightArgs, i: usize, j: &crate::backend::JudgeInfo) -> AnyElement {
    let t = args.t;
    div()
        .id(("judge-row", i as u64))
        .flex()
        .items_center()
        .gap_1p5()
        .px_1p5()
        .py_1()
        .rounded(px(6.))
        .child(
            div()
                .size(px(7.))
                .rounded_full()
                .bg(if j.enabled { t.accent } else { t.dim }),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(11.))
                .text_color(if j.enabled { t.text } else { t.weak })
                .child(j.name.clone()),
        )
        .children((!j.model.is_empty()).then(|| {
            div()
                .px_1()
                .rounded(px(4.))
                .bg(t.well)
                .font_family("JetBrains Mono")
                .text_size(px(8.5))
                .text_color(t.weak)
                .child(j.model.clone())
        }))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn hint(t: &Tokens, text: &str) -> AnyElement {
    div()
        .px_1()
        .py_1()
        .text_size(px(10.))
        .text_color(t.dim)
        .child(text.to_string())
        .into_any_element()
}

/// A wing name shortened for the narrow rail: keep the namespace + last path
/// segment (`repo:/a/b/foo` -> `repo:foo`), leave `agent:`/`user:` intact.
fn wing_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("repo:") {
        let tail = rest.rsplit('/').next().unwrap_or(rest);
        format!("repo:{tail}")
    } else {
        name.to_string()
    }
}

/// First non-empty line of a drawer's content, for the collapsed preview.
fn first_line(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Relative timestamp from a kennel ISO-8601 UTC string (`YYYY-MM-DDTHH:MM:SS`
/// [+00:00]). Falls back to the raw date on a parse miss — honesty over a
/// fabricated "now".
fn rel_ts(ts: &str) -> String {
    match (iso_to_unix(ts), now_unix()) {
        (Some(then), Some(now)) if now >= then => fmt_ago((now - then) as u64),
        _ => ts.split('T').next().unwrap_or(ts).to_string(),
    }
}

fn now_unix() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Minimal UTC ISO-8601 -> unix seconds (kennel writes `timespec=seconds`).
/// Ignores any timezone suffix (the kennel stamps UTC, confirmed by +00:00).
fn iso_to_unix(ts: &str) -> Option<i64> {
    let (date, time) = ts.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: i64 = d.next()?.parse().ok()?;
    let da: i64 = d.next()?.parse().ok()?;
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '-']).next().unwrap_or(time);
    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // Days since the Unix epoch via the civil-from-days algorithm (Howard
    // Hinnant's `days_from_civil`).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_parse_matches_known_epoch() {
        // 2026-06-15T17:19:57+00:00 — verified against Python's datetime.
        assert_eq!(iso_to_unix("2026-06-15T17:19:57+00:00"), Some(1781543997));
        // Epoch itself.
        assert_eq!(iso_to_unix("1970-01-01T00:00:00Z"), Some(0));
        // Bad input falls through to None (caller shows the raw date).
        assert_eq!(iso_to_unix("not-a-timestamp"), None);
    }

    #[test]
    fn wing_label_shortens_repo_only() {
        assert_eq!(
            wing_label("repo:/Users/jacob/dev/puppy-home"),
            "repo:puppy-home"
        );
        assert_eq!(wing_label("user:default"), "user:default");
        assert_eq!(wing_label("agent:code-puppy"), "agent:code-puppy");
    }

    #[test]
    fn first_line_skips_blanks() {
        assert_eq!(first_line("\n\n  hello world  \nmore"), "hello world");
        assert_eq!(first_line(""), "");
    }
}
