//! Small GPUI building blocks for the dashboard: number formatting, the
//! sparkline canvas, pulsing status dots, buttons, and toast data.
//!
//! Format helpers mirror redesign/egui's `views/widgets.rs` exactly — the two
//! branches must print the same numbers.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Div, FontWeight, IntoElement, Path, Render,
    Rgba, Window, canvas, div, ease_in_out, point, prelude::*, px,
};

use super::tokens::Tokens;

/// Card-shaped drag preview (#5 feedback): dragging a dashboard card or list
/// row should look like you're carrying the CARD, not the little tab pill.
/// A compact avatar + name + status-dot rendition that reads as the real one.
pub struct CardGhost {
    pub t: Tokens,
    pub emoji: String,
    pub name: String,
    pub label: String,
    pub color: Rgba,
}

impl Render for CardGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        div()
            .w(px(248.))
            .flex()
            .items_center()
            .gap_2p5()
            .p_3()
            .rounded(px(13.))
            .bg(t.card)
            .border_1()
            .border_color(alpha(self.color, 0.6))
            .shadow_lg()
            .child(
                div()
                    .size(px(34.))
                    .flex_none()
                    .rounded(px(11.))
                    .bg(t.well)
                    .border_1()
                    .border_color(alpha(self.color, 0.5))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(17.))
                    .child(self.emoji.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child(self.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().size(px(7.)).rounded_full().bg(self.color))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(self.color)
                                    .child(self.label.clone()),
                            ),
                    ),
            )
    }
}

/// A lightweight floating preview rendered under the cursor while a tab or
/// dashboard card is being dragged (#5). gpui's `on_drag` wants an
/// `Entity<impl Render>`, so this is the smallest thing that satisfies it:
/// the dragged workspace's color dot + name in a little floating pill.
pub struct DragGhost {
    pub t: Tokens,
    pub label: String,
    pub color: Rgba,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.t;
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(px(8.))
            .bg(t.card)
            .border_1()
            .border_color(alpha(t.accent, 0.8))
            .shadow_lg()
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().size(px(7.)).rounded_full().bg(self.color))
            .child(self.label.clone())
    }
}

/// "41k" token formatting: 999 → "999", 1k–10k one decimal, then whole k / M.
pub fn fmt_k(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1_000.0),
        10_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Elapsed turn clock: "47s" under a minute, then "3:04".
pub fn fmt_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// "How long ago": "32s ago" / "4m ago" / "2h ago".
pub fn fmt_ago(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// A color with its alpha replaced (gpui `Rgba` is a plain struct).
pub fn alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

/// A transient bottom-center notification. Pruned by the drain loop.
pub struct Toast {
    pub msg: String,
    pub color: Rgba,
    pub born: Instant,
}

/// How long a toast stays up (mock: 2.2s).
pub const TOAST_TTL: Duration = Duration::from_millis(2200);

/// An area sparkline painted with `canvas`: a soft fill under the curve plus
/// a brighter top band. Same normalization as the egui painter (0..max).
pub fn sparkline(data: Vec<f32>, w: f32, h: f32, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if data.len() < 2 {
                return;
            }
            let max = data.iter().fold(1.0_f32, |m, &v| m.max(v));
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let step = bw / (data.len() - 1) as f32;
            let y_of = |v: f32| oy + bh - (v / max).clamp(0.0, 1.0) * (bh - 2.0) - 1.0;

            // Soft area fill under the curve.
            let base = oy + bh;
            let mut fill = Path::new(point(px(ox), px(base)));
            for (i, &v) in data.iter().enumerate() {
                fill.line_to(point(px(ox + step * i as f32), px(y_of(v))));
            }
            fill.line_to(point(px(ox + bw), px(base)));
            window.paint_path(fill, alpha(color, 0.16));

            // The line itself: forward along the points, back along a 1.4px
            // vertical offset (a filled band reads as a stroke).
            let mut line = Path::new(point(px(ox), px(y_of(data[0]))));
            for (i, &v) in data.iter().enumerate().skip(1) {
                line.line_to(point(px(ox + step * i as f32), px(y_of(v))));
            }
            for (i, &v) in data.iter().enumerate().rev() {
                line.line_to(point(px(ox + step * i as f32), px(y_of(v) + 1.4)));
            }
            window.paint_path(line, color);
        },
    )
    .w(px(w))
    .h(px(h))
}

/// An 8px state dot; live dots get an expanding halo pulse (gated off by
/// reduce-motion, which renders a static soft ring instead).
pub fn status_dot(id: u64, color: Rgba, live: bool, reduce_motion: bool) -> Div {
    let dot = div()
        .size(px(8.))
        .flex_none()
        .rounded_full()
        .bg(color)
        .relative();
    if !live {
        return dot;
    }
    let halo = div()
        .absolute()
        .inset_0()
        .rounded_full()
        .border_1()
        .border_color(alpha(color, 0.9));
    if reduce_motion {
        return dot.child(halo.inset(px(-3.)).border_color(alpha(color, 0.35)));
    }
    dot.child(
        halo.with_animation(
            ("dot-pulse", id),
            Animation::new(Duration::from_millis(1600))
                .repeat()
                .with_easing(ease_in_out),
            |el, delta| {
                let t = 1.0 - (delta * 2.0 - 1.0).abs(); // 0→1→0 loop
                el.inset(px(-1.0 - 4.0 * t)).opacity(1.0 - t * 0.85)
            },
        )
        .into_any_element(),
    )
}

/// A three-dot "connecting" spinner for workspaces still spinning up their
/// sidecar (#4). The dots pulse in a staggered wave; reduce-motion collapses
/// it to three dim static dots (still reads as "in progress").
pub fn spinner(t: &Tokens, id: u64, reduce_motion: bool) -> AnyElement {
    let color = t.accent;
    let dot = move |i: u64| {
        let d = div().size(px(5.)).rounded_full().bg(color);
        if reduce_motion {
            return d.opacity(0.55).into_any_element();
        }
        d.with_animation(
            ("spin-dot", id * 8 + i),
            Animation::new(Duration::from_millis(1000)).repeat(),
            move |el, delta| {
                // Each dot is a third of a cycle behind the previous one.
                let phase = (delta + i as f32 / 3.0) % 1.0;
                let tri = 1.0 - (phase * 2.0 - 1.0).abs(); // 0→1→0
                el.opacity(0.25 + 0.75 * tri)
            },
        )
        .into_any_element()
    };
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .child(dot(0))
        .child(dot(1))
        .child(dot(2))
        .into_any_element()
}

/// Neutral card-style button shell; caller adds `.id(...)` + `.on_click(...)`.
pub fn btn(t: &Tokens, label: impl Into<String>) -> Div {
    div()
        .px_2p5()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .text_size(px(12.))
        .text_color(t.text)
        .cursor_pointer()
        .hover(|s| s.border_color(alpha(t.accent, 0.6)))
        .child(label.into())
}

/// A [`btn`] with a leading monochrome SVG icon (tinted to the text color).
/// `icon` is an asset path like `"icons/whistle.svg"`.
pub fn icon_btn(t: &Tokens, icon: &'static str, label: impl Into<String>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2p5()
        .py_1()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(t.line_soft)
        .text_size(px(12.))
        .text_color(t.text)
        .cursor_pointer()
        .hover(|s| s.border_color(alpha(t.accent, 0.6)))
        .child(
            gpui::svg()
                .path(icon)
                .size(px(13.))
                .text_color(t.text)
                .flex_none(),
        )
        .child(label.into())
}

/// Accent-filled call-to-action (ink-on-amber), mirror of egui's primary_btn.
pub fn primary_btn(t: &Tokens, label: impl Into<String>) -> Div {
    div()
        .px_2p5()
        .py_1()
        .rounded(px(8.))
        .bg(t.accent)
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(t.accent_ink)
        .cursor_pointer()
        .hover(|s| s.bg(t.accent_2))
        .child(label.into())
}

/// A plain-text hover tooltip view (gpui tooltips are views; this is the
/// minimal one — used for "hover for full prompt"). Tokens resolved once at
/// build time, not re-parsed from the palette every tooltip frame.
pub struct TextTip(pub String, Tokens);

impl gpui::Render for TextTip {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let t = self.1;
        div()
            .max_w(px(420.))
            .px_2p5()
            .py_1p5()
            .rounded(px(8.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            .text_size(px(12.))
            .text_color(t.text)
            .child(self.0.clone())
    }
}

/// Tooltip builder for [`gpui::InteractiveElement::tooltip`]-style hooks.
pub fn text_tip(
    text: String,
) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    // Tooltips render in their own root — the app's text-color cascade
    // doesn't reach them; resolve the ACTIVE theme, not a stale dark
    // constant (B13.2).
    move |_, cx| {
        let tokens = Tokens::current();
        cx.new(|_| TextTip(text.clone(), tokens)).into()
    }
}

/// The toast layer (bottom-center, painted above everything).
pub fn toast_layer(t: &Tokens, toasts: &[Toast]) -> AnyElement {
    if toasts.is_empty() {
        return div().into_any_element();
    }
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_6()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .children(toasts.iter().map(|toast| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded(px(10.))
                .bg(t.panel)
                .border_1()
                .border_color(t.line_soft)
                .shadow_lg()
                .text_size(px(12.5))
                .text_color(t.text)
                .child(div().size(px(8.)).rounded_full().bg(toast.color))
                .child(toast.msg.clone())
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_k_breakpoints() {
        assert_eq!(fmt_k(0), "0");
        assert_eq!(fmt_k(999), "999");
        assert_eq!(fmt_k(1_000), "1.0k");
        assert_eq!(fmt_k(9_999), "10.0k");
        assert_eq!(fmt_k(41_200), "41k");
        assert_eq!(fmt_k(1_200_000), "1.2M");
    }

    #[test]
    fn fmt_elapsed_minutes_roll() {
        assert_eq!(fmt_elapsed(47), "47s");
        assert_eq!(fmt_elapsed(184), "3:04");
        assert_eq!(fmt_elapsed(60), "1:00");
    }

    #[test]
    fn fmt_ago_units() {
        assert_eq!(fmt_ago(32), "32s ago");
        assert_eq!(fmt_ago(240), "4m ago");
        assert_eq!(fmt_ago(7200), "2h ago");
    }

    #[test]
    fn alpha_only_touches_alpha() {
        let c = gpui::rgb(0xe7ab4d);
        let a = alpha(c, 0.5);
        assert_eq!((a.r, a.g, a.b), (c.r, c.g, c.b));
        assert_eq!(a.a, 0.5);
    }
}
