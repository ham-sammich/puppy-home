//! Den roster: members grouped with owner-colored avatars, you/host tags,
//! presence dots, and compact RoomAgent cards (state, agent/model/dir,
//! tok/s + locally-derived sparkline, verb+file, +A −D, Open for your own
//! agents, Nudge for everyone else's puppy).

use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px,
};

use puppy_relay::protocol::{Presence, RoomAgentInfo};

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, tokens};

use super::{DenAction, DenArgs};

pub fn roster_panel(args: &DenArgs) -> AnyElement {
    let t = args.t;
    div()
        .id("den-roster-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2p5()
        .children(args.den.state.members.iter().map(|m| {
            let color = tokens::hex(&m.color);
            let me = m.user == args.den.user;
            let agents = args
                .den
                .state
                .roster
                .get(&m.user)
                .map(|(a, _)| a.as_slice())
                .unwrap_or(&[]);
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .p_2p5()
                .rounded(px(12.))
                .bg(t.card)
                .border_1()
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(26.))
                                .rounded_full()
                                .bg(alpha(color, 0.18))
                                .border_1()
                                .border_color(alpha(color, 0.8))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.))
                                .child("\u{1f436}"),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.))
                                .text_color(color)
                                .child(m.user.clone()),
                        )
                        .children(me.then(|| tag(&t, "you")))
                        .children(m.host.then(|| tag(&t, "host")))
                        .children((!m.puppy.is_empty()).then(|| {
                            div()
                                .font_family("JetBrains Mono")
                                .text_size(px(11.))
                                .text_color(t.weak)
                                .child(format!("\u{1f415} {}", m.puppy))
                        }))
                        .child(div().flex_1())
                        .child(div().size(px(8.)).rounded_full().bg(match m.presence {
                            Presence::Active => t.run,
                            Presence::Idle => t.dim,
                        }))
                        .child(div().text_size(px(10.5)).text_color(t.weak).child(
                            match m.presence {
                                Presence::Active => "active",
                                Presence::Idle => "idle",
                            },
                        )),
                )
                .children(if agents.is_empty() {
                    vec![
                        div()
                            .text_size(px(11.))
                            .text_color(t.dim)
                            .child("no agents reported yet")
                            .into_any_element(),
                    ]
                } else {
                    agents
                        .iter()
                        .map(|a| room_agent(args, &m.user, me, a))
                        .collect()
                })
        }))
        .children(args.den.state.members.is_empty().then(|| {
            div()
                .text_size(px(12.))
                .text_color(t.weak)
                .child("Nobody here yet \u{2014} Invite someone!")
        }))
        .into_any_element()
}

fn tag(t: &crate::gpui_ui::Tokens, label: &str) -> AnyElement {
    div()
        .px_1p5()
        .rounded_full()
        .bg(t.well)
        .text_size(px(9.5))
        .text_color(t.weak)
        .child(label.to_string())
        .into_any_element()
}

/// One compact agent card inside a member group.
fn room_agent(args: &DenArgs, user: &str, mine: bool, a: &RoomAgentInfo) -> AnyElement {
    let t = args.t;
    let state_color = match a.state.as_str() {
        "running" => t.run,
        "thinking" | "tool" => t.think,
        "waiting for input" => t.wait,
        "paused" => t.paused,
        "dead" => t.error,
        _ => t.dim,
    };
    let sparks = args
        .den
        .sparks
        .get(&(user.to_string(), a.dir.clone()))
        .map(|r| r.samples().to_vec())
        .unwrap_or_default();
    let verb_line = match (a.verb.is_empty(), a.file.is_empty()) {
        (false, false) => format!("{} {}", a.verb, a.file),
        (false, true) => a.verb.clone(),
        (true, false) => a.file.clone(),
        (true, true) => String::new(),
    };

    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded(px(9.))
        .bg(t.well)
        .child(
            div()
                .size(px(7.))
                .flex_none()
                .rounded_full()
                .bg(state_color),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(a.dir.clone()),
                        )
                        .child(
                            div()
                                .font_family("JetBrains Mono")
                                .text_size(px(10.))
                                .text_color(t.weak)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(format!("{} \u{b7} {}", a.agent, a.model)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(10.5))
                        .child(div().text_color(state_color).child(a.state.clone()))
                        .children((!verb_line.is_empty()).then(|| {
                            div()
                                .font_family("JetBrains Mono")
                                .text_color(t.weak)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(verb_line)
                        })),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .font_family("JetBrains Mono")
                .text_size(px(10.5))
                .child(div().text_color(t.text).child(format!("{:.0} t/s", a.tps)))
                .child(widgets::sparkline(sparks, 40.0, 14.0, t.run))
                .child(div().text_color(t.run).child(format!("+{}", a.added)))
                .child(
                    div()
                        .text_color(t.error)
                        .child(format!("\u{2212}{}", a.removed)),
                ),
        )
        .children(mine.then(|| {
            let root = args.root.clone();
            let dir = a.dir.clone();
            widgets::btn(&t, "Open")
                .id(("den-open", a.dir.len() as u64))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(DenAction::OpenOwn(dir.clone())), cx)
                    });
                })
                .into_any_element()
        }))
        .children((!mine && !a.puppy.is_empty()).then(|| {
            let root = args.root.clone();
            let puppy = a.puppy.clone();
            widgets::btn(&t, "\u{1f44b} Nudge")
                .id(("den-nudge", a.dir.len() as u64))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(DenAction::Nudge(puppy.clone())), cx)
                    });
                })
                .into_any_element()
        }))
        .into_any_element()
}
