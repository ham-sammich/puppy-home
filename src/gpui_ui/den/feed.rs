//! The Den's coordination feed (right column, 340px): human / puppy / system
//! entries with owner colors and review badges, bottom-pinned scroll
//! (`flex_col_reverse`), a 150-entry render tail + Show older, and the
//! message composer at the bottom.

use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px,
};

use puppy_relay::protocol::{FeedEntry, FeedKind};

use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, tokens};

use super::{DenAction, DenArgs, member_color};

/// Max feed entries rendered unless "Show older" was clicked.
const FEED_TAIL: usize = 150;

pub fn feed_panel(args: &DenArgs) -> AnyElement {
    let t = args.t;
    let den = args.den;
    let total = den.state.feed.len();
    let start = if args.show_all_feed {
        0
    } else {
        total.saturating_sub(FEED_TAIL)
    };

    // Newest-first children inside a reversed column = pinned to the bottom.
    let mut rows: Vec<AnyElement> = Vec::with_capacity(total - start + 1);
    for entry in den.state.feed.iter().skip(start).rev() {
        rows.push(feed_row(args, entry));
    }
    if start > 0 {
        let root = args.root.clone();
        rows.push(
            div()
                .id("den-feed-older")
                .text_size(px(10.5))
                .text_color(t.weak)
                .cursor_pointer()
                .hover(|d| d.text_color(t.text))
                .child(format!("{start} older \u{2014} show all"))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::Den(DenAction::FeedShowOlder), cx)
                    });
                })
                .into_any_element(),
        );
    }

    div()
        .w(px(340.))
        .flex_none()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .overflow_hidden()
        .child(
            div()
                .px_2p5()
                .py_1()
                .border_b_1()
                .border_color(t.line_soft)
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.weak)
                .child("FEED"),
        )
        .child(
            div()
                .id("den-feed-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col_reverse()
                .gap_1p5()
                .px_2p5()
                .py_2()
                .children(rows),
        )
        .child(feed_composer(args))
        .into_any_element()
}

fn feed_row(args: &DenArgs, entry: &FeedEntry) -> AnyElement {
    let t = args.t;
    match entry.kind {
        FeedKind::System => div()
            .text_size(px(10.5))
            .text_color(t.dim)
            .italic()
            .child(entry.text.clone())
            .into_any_element(),
        FeedKind::Human => {
            let color = tokens::hex(member_color(args.den, &entry.user));
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(entry.user.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text)
                        .child(entry.text.clone()),
                )
                .into_any_element()
        }
        FeedKind::Puppy => {
            let color = tokens::hex(member_color(args.den, &entry.user));
            let mut who = format!("\u{1f415} {}", entry.puppy);
            if !entry.to_puppy.is_empty() {
                who.push_str(&format!(" \u{2192} {}", entry.to_puppy));
            }
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(alpha(color, 0.07))
                .border_l_2()
                .border_color(alpha(color, 0.7))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .font_family("JetBrains Mono")
                                .text_size(px(10.))
                                .text_color(color)
                                .child(who),
                        )
                        .children(entry.review.then(|| {
                            div()
                                .px_1p5()
                                .rounded_full()
                                .bg(alpha(t.think, 0.18))
                                .text_size(px(9.))
                                .text_color(t.think)
                                .child("review")
                        })),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(t.text)
                        .child(entry.text.clone()),
                )
                .into_any_element()
        }
    }
}

fn feed_composer(args: &DenArgs) -> AnyElement {
    let t = args.t;
    let Some(input) = args.feed_input else {
        return div().into_any_element();
    };
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_1p5()
        .border_t_1()
        .border_color(t.line_soft)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .child((*input).clone()),
        )
        .child(
            widgets::primary_btn(&t, "Send")
                .id("den-feed-send")
                .on_click({
                    let root = args.root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::Den(DenAction::FeedSend), cx)
                        });
                    }
                }),
        )
        .into_any_element()
}
