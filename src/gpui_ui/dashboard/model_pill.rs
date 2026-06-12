//! The card's model pill + its "switch model · live" popover (deferred so it
//! paints above sibling cards; closes on outside click).

use gpui::{Entity, IntoElement, ParentElement as _, Styled as _, Window, div, prelude::*, px};

use crate::gpui_ui::widgets::alpha;
use crate::gpui_ui::{DashAction, RootView, Tokens};

use super::CardSnapshot;

/// The model pill + its switch-live popover (anchored, deferred above).
pub fn model_pill(
    t: &Tokens,
    s: &CardSnapshot,
    root_entity: &Entity<RootView>,
) -> impl IntoElement {
    let t = *t;
    let id = s.id;
    let pill = div()
        .id(("model-pill", id.0))
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .font_family("JetBrains Mono")
        .text_size(px(11.))
        .text_color(t.weak)
        .cursor_pointer()
        .hover(|st| st.border_color(alpha(t.accent, 0.6)).text_color(t.text))
        .max_w(px(180.))
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(s.model.clone())
        .on_click({
            let root = root_entity.clone();
            move |_, _: &mut Window, cx: &mut gpui::App| {
                root.update(cx, |r, cx| r.dispatch(DashAction::TogglePopover(id), cx));
            }
        });

    let Some(catalog) = &s.catalog else {
        return div().child(pill).into_any_element();
    };

    // Popover: deferred so it paints above sibling cards.
    let root = root_entity.clone();
    let current = s.model.clone();
    let panel = div()
        .occlude()
        .absolute()
        .top(px(26.))
        .right_0()
        .min_w(px(230.))
        .max_h(px(280.))
        .id(("model-pop-scroll", id.0))
        .overflow_y_scroll()
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
            let root = root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::ClosePopover, cx));
            }
        })
        .child(
            div()
                .px_2()
                .py_0p5()
                .text_size(px(10.))
                .text_color(t.weak)
                .child("Switch model \u{b7} live"),
        )
        .children(if catalog.is_empty() {
            vec![
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .child("model catalog not loaded yet")
                    .into_any_element(),
            ]
        } else {
            catalog
                .iter()
                .enumerate()
                .map(|(i, (name, desc))| {
                    let sel = *name == current;
                    let root = root.clone();
                    let model = name.clone();
                    div()
                        .id(("model-opt", i as u64))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(px(7.))
                        .cursor_pointer()
                        .when(sel, |d| d.bg(alpha(t.accent, 0.12)))
                        .hover(|d| d.bg(t.well))
                        .child(
                            div()
                                .font_family("JetBrains Mono")
                                .text_size(px(11.5))
                                .text_color(t.text)
                                .child(name.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(t.dim)
                                .max_w(px(110.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(desc.clone()),
                        )
                        .when(sel, |d| {
                            d.child(
                                div()
                                    .text_color(t.accent)
                                    .text_size(px(11.))
                                    .child("\u{2713}"),
                            )
                        })
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::SetModel(id, model.clone()), cx)
                            });
                        })
                        .into_any_element()
                })
                .collect()
        });

    div()
        .relative()
        .child(pill)
        .child(gpui::deferred(panel).with_priority(100))
        .into_any_element()
}
