//! The performance HUD — egui `src/perf.rs` translated to retained mode.
//!
//! What maps: rolling avg/max cost vs the 16.7ms (60fps) budget, rate,
//! process memory (Windows; zeros elsewhere, same as egui), uptime. What
//! doesn't: eframe's `cpu_usage` covers the whole frame; here the only
//! cost the shell can see is **building the element tree** in
//! `RootView::render` — GPUI's own layout/paint happens after, so the
//! label says "render build" and the footnote owns the difference.
//! "renders / sec" replaces "repaints / sec" (same demand-not-cap caveat:
//! the drain loop notifies 1-4x/s idle; input/streaming push it up).
//!
//! Dev-toggle obscure, like egui's menu item: click the toolbar's
//! fleet-stats text ("N agents running · ...") to toggle.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Rgba, Styled as _, div,
    prelude::*, px,
};

use crate::gpui_ui::widgets::alpha;
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::perf::{WINDOW, fmt_bytes, mean, peak, process_memory, push};

/// Rolling GPUI-shell render stats; sampled by `RootView::render`.
pub struct GpuiPerf {
    pub visible: bool,
    started: Instant,
    last_render: Option<Instant>,
    /// Wall-clock gap between successive renders (demand, not a cap).
    render_gaps: VecDeque<f32>,
    /// Seconds spent building each element tree.
    build_costs: VecDeque<f32>,
    mem_polled: Option<Instant>,
    working_set: u64,
    private_bytes: u64,
}

impl Default for GpuiPerf {
    fn default() -> Self {
        Self {
            visible: false,
            started: Instant::now(),
            last_render: None,
            render_gaps: VecDeque::with_capacity(WINDOW),
            build_costs: VecDeque::with_capacity(WINDOW),
            mem_polled: None,
            working_set: 0,
            private_bytes: 0,
        }
    }
}

impl GpuiPerf {
    /// Call at the top of `render`; returns the stamp for `frame_end`.
    pub fn frame_begin(&mut self) -> Instant {
        let now = Instant::now();
        if let Some(last) = self.last_render.replace(now) {
            push(&mut self.render_gaps, (now - last).as_secs_f32());
        }
        // Memory is an OS call — poll at 1Hz, only while the HUD is open.
        if self.visible
            && self
                .mem_polled
                .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.mem_polled = Some(now);
            let (ws, pv) = process_memory();
            self.working_set = ws;
            self.private_bytes = pv;
        }
        now
    }

    /// Call once the element tree is built.
    pub fn frame_end(&mut self, began: Instant) {
        push(&mut self.build_costs, began.elapsed().as_secs_f32());
    }
}

/// The HUD overlay, anchored top-right (egui's window anchor).
pub(crate) fn hud(t: &Tokens, root: &Entity<RootView>, perf: &GpuiPerf) -> AnyElement {
    let avg_ms = mean(&perf.build_costs) * 1000.0;
    let max_ms = peak(&perf.build_costs) * 1000.0;
    let budget = 1000.0 / 60.0;
    let rps = match mean(&perf.render_gaps) {
        g if g > 0.0 => 1.0 / g,
        _ => 0.0,
    };
    let cost_color = |ms: f32| -> Rgba {
        if ms > budget {
            t.error
        } else if ms > budget * 0.5 {
            t.paused
        } else {
            gpui::rgb(0x8cc88c)
        }
    };
    let row = |label: &str, value: String, color: Option<Rgba>| {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(150.))
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .text_color(color.unwrap_or(t.text))
                    .child(value),
            )
    };

    let s = perf.started.elapsed().as_secs();
    let mut panel = div()
        .occlude()
        .absolute()
        .top(px(44.))
        .right(px(12.))
        .w(px(280.))
        .flex()
        .flex_col()
        .gap_1()
        .p_2p5()
        .rounded(px(10.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child("Performance"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("perf-close")
                        .px_1p5()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(t.weak)
                        .hover(|d| d.bg(t.well))
                        .child("\u{2715}")
                        .on_click({
                            let root = root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| r.dispatch(DashAction::PerfToggle, cx));
                            }
                        }),
                ),
        )
        .child(row(
            "render build (avg)",
            format!("{avg_ms:.2} ms"),
            Some(cost_color(avg_ms)),
        ))
        .child(row(
            "render build (max)",
            format!("{max_ms:.2} ms"),
            Some(cost_color(max_ms)),
        ))
        .child(row("renders / sec", format!("{rps:.1}"), None));
    if perf.working_set > 0 {
        panel = panel
            .child(row(
                "memory (working set)",
                fmt_bytes(perf.working_set),
                None,
            ))
            .child(row("memory (private)", fmt_bytes(perf.private_bytes), None));
    }
    panel = panel
        .child(row(
            "uptime",
            format!("{}h {:02}m {:02}s", s / 3600, (s / 60) % 60, s % 60),
            None,
        ))
        .child(
            div()
                .text_size(px(10.5))
                .text_color(alpha(t.weak, 0.8))
                .child(
                    "Event-driven UI: renders/sec is demand, not a cap. Cost is the \
                     element-tree build; GPUI layout/paint runs after it.",
                ),
        );

    gpui::deferred(panel).with_priority(240).into_any_element()
}
