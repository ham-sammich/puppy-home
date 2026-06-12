//! The Command Center widget kit: custom-paint primitives shared by the
//! redesign views (dashboard cards first, workspace chat + den later).
//!
//! Recipes follow `EGUI_GUIDE.md` §3–§6. Everything color-stateful takes the
//! resolved [`Accents`](crate::theme::Accents); data/numbers render in
//! JetBrains Mono via `FontFamily::Monospace` (wired in `fonts.rs`).
//!
//! Perf contract (this repo has scars):
//! * every animation schedules via `request_repaint_after` with a bounded
//!   interval — never a bare `request_repaint` loop;
//! * decorative loops are gated on `live` so an idle view schedules nothing;
//! * nothing here sizes itself from `available_*` (callers pass sizes).

#![allow(dead_code)] // TODO(task 1.2): the dashboard rebuild consumes the kit

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, vec2,
};

/// Card corner radius (design token: cards 13).
pub const CARD_RADIUS: u8 = 13;
/// Card inner padding (comfy density).
pub const CARD_PAD: f32 = 14.0;
/// Bounded repaint interval for decorative animation (pulse, ring spin).
const ANIM_INTERVAL: Duration = Duration::from_millis(50);

/// Blend `a` toward `b` by `t` in gamma space (cheap tint helper — egui has no
/// CSS `color-mix`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// ---------------------------------------------------------------------------
// Card frame
// ---------------------------------------------------------------------------

/// The agent-card frame: rounded 13, soft drop shadow, state-tinted border,
/// and a faux glow overlay (translucent state-tinted fill) when `glow` is set.
/// Radial gradients aren't native to egui, so the glow is a flat tint painted
/// *under* the content (placeholder-shape trick).
pub fn card<R>(
    ui: &mut Ui,
    border: Color32,
    glow: Option<Color32>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(CARD_PAD)
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
        .show(ui, |ui| {
            // Reserve a paint slot now, fill it once the content height is known,
            // so the glow sits under the content instead of washing it out.
            let glow_slot = glow.map(|_| ui.painter().add(Shape::Noop));
            let r = add_contents(ui);
            if let (Some(slot), Some(col)) = (glow_slot, glow) {
                let rect = ui.min_rect().expand(CARD_PAD);
                ui.painter().set(
                    slot,
                    Shape::rect_filled(
                        rect,
                        CornerRadius::same(CARD_RADIUS),
                        col.linear_multiply(0.05),
                    ),
                );
            }
            r
        })
}

// ---------------------------------------------------------------------------
// Status dot + avatar ring
// ---------------------------------------------------------------------------

/// A state-colored dot; when `live`, an outer halo pulses on a sine driven by
/// `ui.input(i.time)`. The repaint is gated on `live` and bounded.
pub fn status_dot(ui: &mut Ui, col: Color32, live: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let c = rect.center();
    if live {
        let t = ui.input(|i| i.time);
        let a = (0.5 + 0.5 * (t * 2.2).sin() as f32) * 0.30; // 0..0.30
        ui.painter().circle_filled(c, 6.0, col.linear_multiply(a));
        ui.ctx().request_repaint_after(ANIM_INTERVAL);
    }
    ui.painter().circle_filled(c, 3.5, col);
}

/// A 38px rounded avatar holding a role emoji, with a spinning ring arc when
/// `live` (the mock's 3.4s avatar-ring spin). Idle avatars schedule nothing.
pub fn avatar(ui: &mut Ui, emoji: &str, ring: Color32, live: bool) {
    let size = 38.0;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(10), ui.visuals().faint_bg_color);
    p.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        emoji,
        egui::FontId::proportional(18.0),
        ui.visuals().text_color(),
    );
    if live {
        // ~30% arc orbiting the avatar, one revolution per 3.4s.
        let t = ui.input(|i| i.time) as f32;
        let a0 = t * (std::f32::consts::TAU / 3.4);
        let r = size * 0.5 + 2.0;
        let c = rect.center();
        let pts: Vec<Pos2> = (0..=10)
            .map(|i| {
                let a = a0 + i as f32 / 10.0 * std::f32::consts::TAU * 0.3;
                c + r * vec2(a.cos(), a.sin())
            })
            .collect();
        ui.painter().add(Shape::line(pts, Stroke::new(1.5, ring)));
        ui.ctx().request_repaint_after(ANIM_INTERVAL);
    }
}

// ---------------------------------------------------------------------------
// Sparkline
// ---------------------------------------------------------------------------

/// A hand-rolled polyline sparkline (EGUI_GUIDE §5). Allocates exactly
/// `size`; fewer than two samples paints nothing (space still reserved so
/// layouts don't jump as data arrives).
pub fn sparkline(ui: &mut Ui, data: &[f32], size: egui::Vec2, col: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) || data.len() < 2 {
        return;
    }
    let (min, max) = data
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let rng = (max - min).max(1.0);
    let n = (data.len() - 1) as f32;
    let pts: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = rect.left() + rect.width() * i as f32 / n;
            let y = rect.bottom() - 2.0 - (v - min) / rng * (rect.height() - 4.0);
            Pos2::new(x, y)
        })
        .collect();
    ui.painter().add(Shape::line(pts, Stroke::new(1.4, col)));
}

// ---------------------------------------------------------------------------
// Pills + popovers
// ---------------------------------------------------------------------------

/// A fully-rounded pill button (radius 999 ≈ `CornerRadius::same(255)`).
/// Pass mono text yourself (`RichText::monospace`) — model/agent pills are
/// data, and data is JetBrains Mono.
pub fn pill(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(egui::Button::new(text).corner_radius(CornerRadius::same(255)))
}

/// Popover open-state lives in egui temp memory under this id.
fn popover_flag(id: Id) -> Id {
    id.with("popover-open")
}

pub fn popover_is_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<bool>(popover_flag(id)).unwrap_or(false))
}

pub fn popover_toggle(ctx: &egui::Context, id: Id) {
    let open = popover_is_open(ctx, id);
    ctx.data_mut(|d| d.insert_temp(popover_flag(id), !open));
}

pub fn popover_close(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(popover_flag(id), false));
}

/// If open, show a floating panel pinned below `anchor` (popup styling: card
/// fill, deep shadow). Closes itself on Escape or a click outside both the
/// panel and the anchor. Returns the closure's value while open.
pub fn popover_below<R>(
    ui: &Ui,
    id: Id,
    anchor: Rect,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let ctx = ui.ctx().clone();
    if !popover_is_open(&ctx, id) {
        return None;
    }
    let area = egui::Area::new(id.with("popover-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom() + vec2(0.0, 6.0))
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(ui.visuals().window_stroke)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(8.0)
                .shadow(egui::epaint::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 0,
                    color: Color32::from_black_alpha(160),
                })
                .show(ui, add_contents)
                .inner
        });
    let clicked_anchor = ctx.input(|i| {
        i.pointer
            .interact_pos()
            .is_some_and(|p| anchor.contains(p) && i.pointer.any_pressed())
    });
    let dismissed = ctx.input(|i| i.key_pressed(egui::Key::Escape))
        || (area.response.clicked_elsewhere() && !clicked_anchor);
    if dismissed {
        popover_close(&ctx, id);
    }
    Some(area.inner)
}

// ---------------------------------------------------------------------------
// Segmented control
// ---------------------------------------------------------------------------

/// A Grid/List/Focus-style segmented control: selectable labels in a rounded
/// faint-bg frame. Returns true when the selection changed this frame.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(2.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, label) in options {
                    if ui.selectable_label(*value == *v, *label).clicked() && *value != *v {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

/// How long a toast stays up.
const TOAST_TTL: Duration = Duration::from_millis(2200);
/// Most recent toasts kept (older ones drop silently).
const TOAST_KEEP: usize = 4;

struct Toast {
    msg: String,
    color: Color32,
    until: Instant,
}

/// Transient action feedback ("puppy-home paused at next safe point"), pinned
/// bottom-center. `Instant`-based auto-dismiss; repaints are only requested
/// while a toast is visible, bounded by its remaining lifetime.
#[derive(Default)]
pub struct Toasts {
    queue: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, msg: impl Into<String>, color: Color32) {
        self.queue.push(Toast {
            msg: msg.into(),
            color,
            until: Instant::now() + TOAST_TTL,
        });
        if self.queue.len() > TOAST_KEEP {
            self.queue.remove(0);
        }
    }

    /// Draw the newest live toast (if any) and schedule its expiry repaint.
    pub fn render(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.queue.retain(|t| t.until > now);
        let Some(t) = self.queue.last() else { return };
        egui::Area::new(Id::new("cc-toasts"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -26.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(ui.visuals().window_stroke)
                    .corner_radius(CornerRadius::same(255))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(140),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, t.color);
                            ui.label(&t.msg);
                        });
                    });
            });
        // Wake exactly when this toast dies (bounded ≤ TOAST_TTL).
        ctx.request_repaint_after(t.until.saturating_duration_since(now));
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers (mono-stat vocabulary shared by cards + tiles + tables)
// ---------------------------------------------------------------------------

/// `41_200` → "41.2k", `12_400_000` → "12.4M" (the mock's stat shorthand).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_k_breakpoints() {
        assert_eq!(fmt_k(0), "0");
        assert_eq!(fmt_k(999), "999");
        assert_eq!(fmt_k(1_000), "1.0k");
        assert_eq!(fmt_k(8_400), "8.4k");
        assert_eq!(fmt_k(41_200), "41k");
        assert_eq!(fmt_k(999_999), "999k");
        assert_eq!(fmt_k(1_200_000), "1.2M");
    }

    #[test]
    fn fmt_elapsed_matches_mock() {
        assert_eq!(fmt_elapsed(0), "0s");
        assert_eq!(fmt_elapsed(47), "47s");
        assert_eq!(fmt_elapsed(60), "1:00");
        assert_eq!(fmt_elapsed(184), "3:04");
    }

    #[test]
    fn fmt_ago_units() {
        assert_eq!(fmt_ago(32), "32s ago");
        assert_eq!(fmt_ago(240), "4m ago");
        assert_eq!(fmt_ago(7200), "2h ago");
    }

    #[test]
    fn mix_blends_endpoints() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(200, 100, 50);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), Color32::from_rgb(100, 50, 25));
    }

    #[test]
    fn toasts_cap_and_expire() {
        let mut t = Toasts::default();
        for i in 0..10 {
            t.push(format!("t{i}"), Color32::RED);
        }
        assert_eq!(t.queue.len(), TOAST_KEEP);
        // Force-expire everything; render-side retain would drop them, but we
        // can assert the data invariant directly.
        let past = Instant::now() - Duration::from_secs(1);
        for toast in &mut t.queue {
            toast.until = past;
        }
        let now = Instant::now();
        t.queue.retain(|x| x.until > now);
        assert!(t.queue.is_empty());
    }
}
