//! Dashboard sections above the fleet: the pack header (H1 + puppy lede +
//! the five stat tiles), the pink attention banner, and the Grid / List /
//! Focus segmented control.

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Rgba, SharedString,
    Styled as _, div, prelude::*, px,
};

use crate::session::DashboardViewMode;
use crate::workspace::WorkspaceId;

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};

use super::FleetStats;

/// H1 + puppy lede on the left, the five stat tiles on the right.
pub fn pack_header(
    t: &Tokens,
    puppy: &str,
    stats: &FleetStats,
    agg_sparks: Vec<f32>,
    root: &Entity<RootView>,
) -> impl IntoElement {
    let lede = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .text_size(px(13.))
        .text_color(t.weak)
        .child(div().text_size(px(15.)).child("\u{1f436}"))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child(puppy.to_string()),
        )
        .child(format!(
            ", spun up across {} {}",
            stats.dirs,
            if stats.dirs == 1 {
                "directory"
            } else {
                "directories"
            }
        ))
        .child(lede_count(t, stats.running, t.run, "on the hunt", true))
        .child(lede_count(t, stats.napping, t.paused, "napping", false))
        .child(lede_count(t, stats.waiting, t.wait, "need you", false))
        .child(lede_count(t, stats.stuck, t.error, "stuck", false));

    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(260.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(22.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text)
                                .child("Running agents"),
                        )
                        .child({
                            // The whistle: summon a fresh puppy at $HOME —
                            // Open Folder minus the dialog.
                            let root = root.clone();
                            widgets::btn(t, "\u{1f4e3} Whistle")
                                .id("dash-whistle")
                                .tooltip(widgets::text_tip(
                                    "Spawn a Code Puppy at your home directory".into(),
                                ))
                                .on_click(move |_, _, cx| {
                                    root.update(cx, |r, cx| {
                                        r.dispatch(
                                            DashAction::OpenHome { to_chat: false },
                                            cx,
                                        )
                                    });
                                })
                        }),
                )
                .child(lede),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap_2()
                .child(stat_tile(
                    t,
                    "Throughput",
                    format!("{:.0} tok/s", stats.tps),
                    Some(t.accent),
                    Some(agg_sparks),
                ))
                .child(stat_tile(
                    t,
                    "Tokens today",
                    widgets::fmt_k(stats.tokens),
                    None,
                    None,
                ))
                .child(stat_tile(
                    t,
                    "Spend today",
                    // NEVER $0.00 while nothing is priced — an honest dash.
                    // "≈" marks sums containing snapshot-priced estimates.
                    match stats.cost {
                        Some(c) if stats.cost_estimated => format!("\u{2248}${c:.2}"),
                        Some(c) => format!("${c:.2}"),
                        None => "\u{2014}".to_string(),
                    },
                    None,
                    None,
                ))
                .child(stat_tile(
                    t,
                    "Tool calls",
                    stats.tools.to_string(),
                    None,
                    None,
                ))
                .child(stat_tile(
                    t,
                    "Errors",
                    stats.stuck.to_string(),
                    (stats.stuck > 0).then_some(t.error),
                    None,
                )),
        )
}

/// One " · N label" lede segment, colored; hidden when zero (except running).
fn lede_count(t: &Tokens, n: usize, color: Rgba, label: &str, always: bool) -> AnyElement {
    if n == 0 && !always {
        return div().into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().text_color(t.dim).child("\u{b7}"))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(n.to_string()),
        )
        .child(label.to_string())
        .into_any_element()
}

/// One header stat tile; `spark` adds the live sparkline (Throughput only).
fn stat_tile(
    t: &Tokens,
    k: &str,
    v: String,
    color: Option<Rgba>,
    spark: Option<Vec<f32>>,
) -> impl IntoElement {
    let border = color.map_or(t.line_soft, |c| alpha(c, 0.45));
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .min_w(px(86.))
        .px_2p5()
        .py_1p5()
        .rounded(px(10.))
        .bg(t.card)
        .border_1()
        .border_color(border)
        .child(
            div()
                .text_size(px(10.))
                .text_color(t.weak)
                .child(k.to_string()),
        )
        .child(
            div()
                .font_family("JetBrains Mono")
                .font_weight(FontWeight::BOLD)
                .text_size(px(14.))
                .text_color(color.unwrap_or(t.text))
                .child(v),
        )
        .children(
            spark.map(|data| widgets::sparkline(data, 104.0, 18.0, color.unwrap_or(t.accent))),
        )
}

/// Pink-bordered banner listing every workspace blocked on input.
pub fn attention_banner(
    t: &Tokens,
    waiting: &[(WorkspaceId, String, Option<String>)],
    root: &Entity<RootView>,
    reduce_motion: bool,
) -> AnyElement {
    if waiting.is_empty() {
        return div().into_any_element();
    }
    let names = waiting
        .iter()
        .map(|(_, n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let verb = if waiting.len() > 1 { "are" } else { "is" };
    let question = (waiting.len() == 1).then(|| waiting[0].2.clone()).flatten();

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(10.))
        .bg(alpha(t.wait, 0.08))
        .border_1()
        .border_color(alpha(t.wait, 0.5))
        .child(widgets::status_dot(u64::MAX, t.wait, true, reduce_motion))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.wait)
                .text_size(px(13.))
                .child(names),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(t.text)
                .child(format!("{verb} waiting on you")),
        )
        .children(question.map(|q| {
            div()
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .text_color(t.weak)
                .child(format!("\u{2014} {q}"))
        }))
        .child(div().flex_1())
        .children(waiting.iter().map(|(id, name, _)| {
            let root = root.clone();
            let id = *id;
            widgets::primary_btn(t, format!("Answer {name} \u{2192}"))
                .id(("attn-answer", id.0))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| r.dispatch(DashAction::Open(id), cx));
                })
        }))
        .into_any_element()
}

/// The Grid / List / Focus segmented control.
pub fn segmented(t: &Tokens, mode: DashboardViewMode, root: &Entity<RootView>) -> impl IntoElement {
    let seg = |label: &str, m: DashboardViewMode, idx: u64| {
        let on = mode == m;
        let root = root.clone();
        div()
            .id(("seg-view", idx))
            .px_2p5()
            .py_0p5()
            .rounded(px(7.))
            .text_size(px(12.))
            .cursor_pointer()
            .when(on, |s| {
                s.bg(t.well)
                    .text_color(t.text)
                    .border_1()
                    .border_color(alpha(t.accent, 0.55))
            })
            .when(!on, |s| {
                s.text_color(t.weak).border_1().border_color(t.panel)
            })
            .child(SharedString::from(label.to_string()))
            .on_click(move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::SetView(m), cx));
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
        .child(seg("\u{25a6} Grid", DashboardViewMode::Grid, 0))
        .child(seg("\u{2630} List", DashboardViewMode::List, 1))
        .child(seg("\u{25f0} Focus", DashboardViewMode::Focus, 2))
}
