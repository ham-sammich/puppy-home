//! The Den: a collaborative relay room. Join screen, room header (LIVE,
//! room code, member counts, Invite/Leave), Roster/Board segmented work
//! area, and the 340px coordination feed. State lives in [`DenConn`] (the
//! relay mirror) + a few RootView fields; every interaction goes through
//! `DashAction::Den(DenAction)` — same funnel as everything else.

pub mod actions;
pub mod board;
pub mod feed;
pub mod roster;

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Entity, FontWeight, IntoElement, ParentElement as _,
    Styled as _, div, ease_in_out, prelude::*, px,
};

use crate::pack::{DEN_LABEL, DenState, PackClient, PackEvent};
use crate::workspace::SparkRing;

use super::input::ChatInput;
use super::widgets::{self, alpha};
use super::{DashAction, RootView, Tokens};
pub use actions::DenAction;

/// A live (or recently died) relay connection + the den mirror.
pub struct DenConn {
    pub client: PackClient,
    pub rx: Receiver<PackEvent>,
    pub state: DenState,
    pub room: String,
    pub addr: String,
    pub user: String,
    /// False once the reader reports Disconnected.
    pub alive: bool,
    /// Locally-derived tok/s history per (user, dir) roster card, fed by
    /// successive roster broadcasts (bounded ring each).
    pub sparks: HashMap<(String, String), SparkRing>,
}

/// Roster / Board segmented state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DenSeg {
    #[default]
    Roster,
    Board,
}

/// Open den popover: a kanban card's menu, or the Share-plan picker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DenPop {
    TaskMenu(u64),
    SharePlan,
}

/// What the den task input is currently editing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskTarget {
    Add(puppy_relay::protocol::TaskColumn),
    Retitle(u64),
}

pub struct DenArgs<'a> {
    pub t: Tokens,
    pub root: Entity<RootView>,
    pub den: &'a DenConn,
    pub seg: DenSeg,
    pub pop: Option<&'a DenPop>,
    pub feed_input: Option<&'a Entity<ChatInput>>,
    pub task_input: Option<&'a Entity<ChatInput>>,
    pub task_target: Option<TaskTarget>,
    pub show_all_feed: bool,
    pub reduce_motion: bool,
    /// Open workspace roots that contain a plans.md (the Share picker).
    pub sharable_plans: Vec<(String, std::path::PathBuf)>,
}

/// The whole den screen (joined state).
pub fn den_screen(args: &DenArgs) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2p5()
        .child(header(args))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(seg_toggle(args))
                        .child(match args.seg {
                            DenSeg::Roster => roster::roster_panel(args),
                            DenSeg::Board => board::board_panel(args),
                        }),
                )
                .child(feed::feed_panel(args)),
        )
        .into_any_element()
}

/// LIVE · Den · room-code · counts · relay · Invite · Leave.
fn header(args: &DenArgs) -> AnyElement {
    let t = args.t;
    let den = args.den;
    let root = &args.root;
    let people = den.state.members.len();
    let puppies = den
        .state
        .members
        .iter()
        .filter(|m| !m.puppy.is_empty())
        .count();
    let project = most_common_dir(den).unwrap_or_else(|| "\u{2014}".to_string());

    let live_dot = {
        let dot = div()
            .size(px(8.))
            .rounded_full()
            .bg(if den.alive { t.run } else { t.error });
        if den.alive && !args.reduce_motion {
            dot.with_animation(
                "den-live-blink",
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_easing(ease_in_out),
                |el, delta| el.opacity(0.35 + 0.65 * (1.0 - (delta * 2.0 - 1.0).abs())),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        }
    };

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2p5()
        .px_3()
        .py_2()
        .rounded(px(12.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .child(live_dot)
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if den.alive { t.run } else { t.error })
                .child(if den.alive { "LIVE" } else { "OFFLINE" }),
        )
        .child(
            div()
                .id("den-room-code")
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_0p5()
                .rounded(px(7.))
                .bg(t.well)
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .text_color(t.text)
                .cursor_pointer()
                .hover(|d| d.border_color(alpha(t.accent, 0.5)))
                .child(format!("\u{1f43e} {DEN_LABEL} \u{b7} {}", den.room))
                .on_click({
                    let root = root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::Den(DenAction::CopyRoom), cx)
                        });
                    }
                }),
        )
        .child(div().text_size(px(12.)).text_color(t.weak).child(format!(
            "{people} {} \u{b7} {puppies} {} \u{b7} working",
            if people == 1 { "person" } else { "people" },
            if puppies == 1 { "puppy" } else { "puppies" },
        )))
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.accent)
                .child(project),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.weak)
                .child("together"),
        )
        .child(div().flex_1())
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(10.5))
                .text_color(t.dim)
                .child(format!("relay {}", den.addr)),
        )
        .child(
            widgets::btn(&t, "\u{ff0b} Invite")
                .id("den-invite")
                .on_click({
                    let root = root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::Den(DenAction::Invite), cx)
                        });
                    }
                }),
        )
        .child(widgets::btn(&t, "Leave den").id("den-leave").on_click({
            let root = root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Den(DenAction::Leave), cx)
                });
            }
        }))
        .into_any_element()
}

fn seg_toggle(args: &DenArgs) -> AnyElement {
    let t = args.t;
    let mk = |label: &str, seg: DenSeg, idx: u64| {
        let on = args.seg == seg;
        let root = args.root.clone();
        div()
            .id(("den-seg", idx))
            .px_2p5()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(12.))
            .cursor_pointer()
            .when(on, |d| {
                d.bg(t.well)
                    .text_color(t.text)
                    .border_1()
                    .border_color(alpha(t.accent, 0.55))
            })
            .when(!on, |d| {
                d.text_color(t.weak).border_1().border_color(t.panel)
            })
            .child(label.to_string())
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| {
                    r.dispatch(DashAction::Den(DenAction::SetSeg(seg)), cx)
                });
            })
    };
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded(px(9.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .w(px(180.))
        .child(mk("Roster", DenSeg::Roster, 0))
        .child(mk("Board", DenSeg::Board, 1))
        .into_any_element()
}

/// The most common roster dir across all members = "the project".
fn most_common_dir(den: &DenConn) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (agents, _) in den.state.roster.values() {
        for a in agents {
            if !a.dir.is_empty() {
                *counts.entry(a.dir.as_str()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(dir, n)| (*n, std::cmp::Reverse(dir.to_string())))
        .map(|(dir, _)| dir.to_string())
}

/// The join screen (not connected): relay address, room code, user name.
pub struct JoinArgs<'a> {
    pub t: Tokens,
    pub root: Entity<RootView>,
    pub addr: &'a Entity<ChatInput>,
    pub room: &'a Entity<ChatInput>,
    pub user: &'a Entity<ChatInput>,
    pub error: Option<String>,
}

pub fn join_screen(args: &JoinArgs) -> AnyElement {
    let t = args.t;
    let field = |label: &str, input: &Entity<ChatInput>| {
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(t.weak)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .px_2p5()
                    .py_1p5()
                    .rounded(px(9.))
                    .bg(t.well)
                    .border_1()
                    .border_color(t.line_soft)
                    .font_family("JetBrains Mono")
                    .child(input.clone()),
            )
    };
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(420.))
                .flex()
                .flex_col()
                .gap_2p5()
                .p_4()
                .rounded(px(13.))
                .bg(t.card)
                .border_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child(format!("\u{1f43e} Join a {DEN_LABEL}")),
                )
                .child(div().text_size(px(12.)).text_color(t.weak).child(
                    "Teammates connect to a relay room and everyone's puppy works \
                     the project together.",
                ))
                .child(field("relay address", args.addr))
                .child(field("room code", args.room))
                .child(field("your name", args.user))
                .children(
                    args.error
                        .clone()
                        .map(|e| div().text_size(px(12.)).text_color(t.error).child(e)),
                )
                .child(
                    widgets::primary_btn(&t, format!("Join {DEN_LABEL} \u{2192}"))
                        .id("den-join")
                        .on_click({
                            let root = args.root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| {
                                    r.dispatch(DashAction::Den(DenAction::JoinSubmit), cx)
                                });
                            }
                        }),
                ),
        )
        .into_any_element()
}

/// Owner chip used by board cards + roster headers.
pub(crate) fn owner_chip(_t: &Tokens, name: &str, color_hex: &str) -> AnyElement {
    let color = super::tokens::hex(color_hex);
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(alpha(color, 0.7))
        .text_size(px(10.))
        .text_color(color)
        .child("\u{1f436}")
        .child(name.to_string())
        .into_any_element()
}

/// `t` shorthand for member color lookup.
pub(crate) fn member_color<'a>(den: &'a DenConn, user: &str) -> &'a str {
    den.state
        .members
        .iter()
        .find(|m| m.user == user)
        .map(|m| m.color.as_str())
        .unwrap_or("#9a9aa8")
}
