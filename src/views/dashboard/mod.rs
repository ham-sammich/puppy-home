//! The Command Center dashboard: every running agent as a live card, with the
//! pack header (puppy lede + fleet stat tiles), the attention banner, and
//! Grid / List / Focus fleet views. Cards live in [`card`].
//!
//! Perf: decorative animation is delegated to the widget kit, which gates all
//! repaints on `live` — an idle dashboard schedules nothing. Column math reads
//! `available_width` once per frame to *lay out* fixed-width cards; nothing
//! here feeds its own size back into a resizable container.

mod card;
mod table;

use eframe::egui::{self, Color32, FontFamily, RichText};

use crate::browser::BrowserManager;
use crate::fonts::FAMILY_GROTESK_BOLD;
use crate::session::DashboardViewMode;
use crate::shell::ShellAction;
use crate::supervisor::Supervisor;
use crate::theme::Accents;
use crate::views::widgets::{self, Toasts};
use crate::workspace::{InstanceStatus, Workspace, WorkspaceId};

/// Card grid: minimum card width before a column drops (mock: minmax(420px,1fr)).
const CARD_MIN_W: f32 = 420.0;
/// Gap between grid cards.
const GRID_GAP: f32 = 14.0;
/// Focus view: single column, capped.
const FOCUS_MAX_W: f32 = 880.0;

/// Which inline input a card has expanded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    Steer,
    Send,
}

/// The one open inline card input (steer / new-prompt) — one at a time.
pub(crate) struct CardInput {
    pub ws: WorkspaceId,
    pub kind: InputKind,
    pub text: String,
    /// Steer delivery: false =  now, true =  queue.
    pub queue: bool,
    /// One-shot: focus the field the first frame it shows.
    pub focus: bool,
}

/// Dashboard view state owned by the app (survives tab switches).
#[derive(Default)]
pub struct DashboardView {
    /// Grid / List / Focus (persisted in `session.json`).
    pub mode: DashboardViewMode,
    pub(crate) toasts: Toasts,
    pub(crate) input: Option<CardInput>,
}

impl DashboardView {
    pub fn with_mode(mode: DashboardViewMode) -> Self {
        DashboardView {
            mode,
            ..Default::default()
        }
    }
}

/// Pack-vocabulary state derived from a workspace: label, color, liveness.
pub(crate) struct CardState {
    pub label: &'static str,
    pub color: Color32,
    pub live: bool,
}

pub(crate) fn card_state(status: InstanceStatus, a: &Accents, weak: Color32) -> CardState {
    let (label, color, live) = match status {
        InstanceStatus::Starting => ("Waking up", weak, false),
        InstanceStatus::Running => ("Fetching", a.run, true),
        InstanceStatus::Thinking => ("Sniffing", a.think, true),
        InstanceStatus::ToolCalling => ("Digging", a.think, true),
        InstanceStatus::WaitingForInput => ("Needs you", a.wait, false),
        InstanceStatus::Paused => ("Napping", a.paused, false),
        InstanceStatus::Idle => ("Resting", weak, false),
        InstanceStatus::Dead => ("Stuck", a.error, false),
    };
    CardState { label, color, live }
}

/// Sort rank: needs-you first, then live, then paused/stuck, then resting.
fn rank(status: InstanceStatus) -> u8 {
    match status {
        InstanceStatus::WaitingForInput => 0,
        InstanceStatus::Running
        | InstanceStatus::Thinking
        | InstanceStatus::ToolCalling
        | InstanceStatus::Starting => 1,
        InstanceStatus::Paused | InstanceStatus::Dead => 2,
        InstanceStatus::Idle => 3,
    }
}

/// The role emoji for an agent name (one puppy, many roles).
pub(crate) fn role_emoji(agent: &str) -> &'static str {
    match agent {
        "planner" => "\u{1f5fa}",   // map
        "reviewer" => "\u{1f50d}",  // magnifier
        "tester" => "\u{1f9ea}",    // test tube
        "docs" => "\u{1f4d6}",      // book
        "architect" => "\u{1f4d0}", // triangle ruler
        _ => "\u{1f415}",           // dog (code-puppy + unknown roles)
    }
}

/// The user's home dir, resolved once (this runs per card per frame — an
/// env lookup + alloc each time would be a needless hot-path cost).
fn home_dir() -> Option<&'static str> {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| h.to_string_lossy().into_owned())
            .filter(|h| !h.is_empty())
    })
    .as_deref()
}

/// Abbreviate the user's home dir as `~` for the card meta line.
pub(crate) fn tilde_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    match home_dir() {
        Some(h) if s.starts_with(h) => format!("~{}", &s[h.len()..]),
        _ => s.into_owned(),
    }
}

/// The user's one puppy name, from the first workspace that learned its real
/// name (falls back to Code Puppy's default "Puppy" until a sidecar reports).
fn puppy_name(sup: &Supervisor) -> String {
    sup.iter()
        .map(|w| w.puppy_name.clone())
        .find(|p| !p.is_empty() && p != "Puppy")
        .unwrap_or_else(|| "Puppy".to_string())
}

pub fn render(
    ui: &mut egui::Ui,
    view: &mut DashboardView,
    sup: &Supervisor,
    browser: &BrowserManager,
    accents: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(10.0);
            header(ui, sup, accents, &puppy_name(sup));
            ui.add_space(10.0);
            attention_banner(ui, sup, accents, actions);

            if sup.is_empty() {
                ui.add_space(28.0);
                ui.vertical_centered(|ui| {
                    ui.weak(format!(
                        "No agents running. Open a folder to send {} out. \u{1f43e}",
                        puppy_name(sup)
                    ));
                });
            } else {
                fleet(ui, view, sup, accents, actions);
            }

            ui.add_space(16.0);
            plugins_section(ui, browser);
        });
    view.toasts.render(ui.ctx());
}

/// Pack header: H1 + lede on the left, the five fleet stat tiles on the right.
fn header(ui: &mut egui::Ui, sup: &Supervisor, a: &Accents, puppy: &str) {
    let mut running = 0usize;
    let mut paused = 0usize;
    let mut waiting = 0usize;
    let mut errors = 0usize;
    let mut tps = 0.0f64;
    let mut tokens = 0u64;
    let mut tools = 0u64;
    let mut cost = 0.0f64;
    let mut cost_known = false;
    let mut cost_estimated = false;
    for ws in sup.iter() {
        match ws.status {
            InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => {
                running += 1;
                tps += ws.token_rate;
            }
            InstanceStatus::Paused => paused += 1,
            InstanceStatus::WaitingForInput => waiting += 1,
            InstanceStatus::Dead => errors += 1,
            InstanceStatus::Starting | InstanceStatus::Idle => {}
        }
        tokens += ws.total_tokens;
        tools += ws.tool_calls;
        if let Some(c) = ws.cost {
            cost += c;
            cost_known = true;
            cost_estimated |= ws.cost_estimated;
        }
    }
    let dirs = sup.len();

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Running agents")
                    .family(FontFamily::Name(FAMILY_GROTESK_BOLD.into()))
                    .size(24.0),
            );
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(format!("\u{1f436} {puppy}, spun up across "));
                ui.strong(dirs.to_string());
                ui.label(if dirs == 1 {
                    " directory"
                } else {
                    " directories"
                });
                ui.label(" · ");
                ui.label(RichText::new(running.to_string()).color(a.run).strong());
                ui.label(" on the hunt");
                if paused > 0 {
                    ui.label(" · ");
                    ui.label(RichText::new(paused.to_string()).color(a.paused).strong());
                    ui.label(" napping");
                }
                if waiting > 0 {
                    ui.label(" · ");
                    ui.label(RichText::new(waiting.to_string()).color(a.wait).strong());
                    ui.label(" need you");
                }
                if errors > 0 {
                    ui.label(" · ");
                    ui.label(RichText::new(errors.to_string()).color(a.error).strong());
                    ui.label(" stuck");
                }
            });
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            // Right-to-left: emit in reverse visual order.
            let err_col = (errors > 0).then_some(a.error);
            stat_tile(ui, "Errors", &errors.to_string(), err_col, None);
            stat_tile(ui, "Tool calls", &tools.to_string(), None, None);
            // NEVER $0.00 while nothing is priced — the dash is honest. "≈"
            // marks sums containing snapshot-priced estimates.
            let spend = if cost_known && cost_estimated {
                format!("≈${cost:.2}")
            } else if cost_known {
                format!("${cost:.2}")
            } else {
                "—".to_string()
            };
            stat_tile(ui, "Spend today", &spend, None, None);
            stat_tile(ui, "Tokens today", &widgets::fmt_k(tokens), None, None);
            stat_tile(
                ui,
                "Throughput",
                &format!("{tps:.0} tok/s"),
                Some(a.accent),
                Some(sup.aggregate_sparks()),
            );
        });
    });
}

/// One header stat tile; `spark` adds the live sparkline (Throughput only).
fn stat_tile(ui: &mut egui::Ui, k: &str, v: &str, color: Option<Color32>, spark: Option<&[f32]>) {
    let border = color.map_or(ui.visuals().widgets.noninteractive.bg_stroke.color, |c| {
        c.linear_multiply(0.45)
    });
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_min_width(86.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(k).small().weak());
                ui.label(
                    RichText::new(v)
                        .family(FontFamily::Name(crate::fonts::FAMILY_JBMONO_BOLD.into()))
                        .size(15.0)
                        .color(color.unwrap_or(ui.visuals().text_color())),
                );
                if let Some(data) = spark {
                    widgets::sparkline(ui, data, egui::vec2(104.0, 18.0), color.unwrap());
                }
            });
        });
}

/// Pink-bordered banner listing every workspace blocked on input, with an
/// "Answer {dir} →" jump per workspace.
fn attention_banner(
    ui: &mut egui::Ui,
    sup: &Supervisor,
    a: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    let waiting: Vec<&Workspace> = sup
        .iter()
        .filter(|w| w.status == InstanceStatus::WaitingForInput)
        .collect();
    if waiting.is_empty() {
        return;
    }
    egui::Frame::new()
        .fill(widgets::mix(ui.visuals().panel_fill, a.wait, 0.10))
        .stroke(egui::Stroke::new(1.0, a.wait.linear_multiply(0.5)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::status_dot(ui, a.wait, true);
                let names: Vec<&str> = waiting.iter().map(|w| w.name.as_str()).collect();
                let verb = if names.len() > 1 { "are" } else { "is" };
                ui.label(RichText::new(names.join(", ")).strong().color(a.wait));
                ui.label(format!(" {verb} waiting on you"));
                if let [only] = waiting.as_slice()
                    && let Some(q) = only.pending_question()
                {
                    ui.label(" — ");
                    ui.label(RichText::new(q).monospace().size(12.0).weak());
                }
                for w in &waiting {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("Answer {} →", w.name)).color(a.accent_ink),
                            )
                            .fill(a.accent)
                            .corner_radius(egui::CornerRadius::same(8)),
                        )
                        .clicked()
                    {
                        actions.push(ShellAction::FocusChat(w.id));
                    }
                }
            });
        });
    ui.add_space(8.0);
}

/// The fleet: view switcher + Grid / Focus card layout or the dense List table.
fn fleet(
    ui: &mut egui::Ui,
    view: &mut DashboardView,
    sup: &Supervisor,
    a: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} agent(s)", sup.len()))
                .weak()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::segmented(
                ui,
                &mut view.mode,
                &[
                    (DashboardViewMode::Grid, "\u{25a6} Grid"),
                    (DashboardViewMode::List, "\u{2630} List"),
                    (DashboardViewMode::Focus, "\u{25f0} Focus"),
                ],
            );
        });
    });
    ui.add_space(8.0);

    let mut fleet: Vec<&Workspace> = sup.iter().collect();
    fleet.sort_by_key(|w| rank(w.status));

    match view.mode {
        DashboardViewMode::List => table::render(ui, &fleet, a, actions),
        DashboardViewMode::Grid | DashboardViewMode::Focus => grid(ui, view, &fleet, a, actions),
    }
}

/// Responsive card grid (`minmax(420px,1fr)`); Focus = one centered column.
fn grid(
    ui: &mut egui::Ui,
    view: &mut DashboardView,
    fleet: &[&Workspace],
    a: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    let avail = ui.available_width();
    let (cols, col_w, pad) = if view.mode == DashboardViewMode::Focus {
        let w = avail.min(FOCUS_MAX_W);
        (1usize, w, (avail - w).max(0.0) / 2.0)
    } else {
        let c = ((avail + GRID_GAP) / (CARD_MIN_W + GRID_GAP))
            .floor()
            .max(1.0) as usize;
        (c, (avail - GRID_GAP * (c - 1) as f32) / c as f32, 0.0)
    };
    for row in fleet.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GRID_GAP;
            if pad > 0.0 {
                ui.add_space(pad);
            }
            for ws in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(col_w, 1.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(col_w);
                        card::agent_card(ui, ws, a, view, actions);
                    },
                );
            }
        });
        ui.add_space(GRID_GAP);
    }
}

/// The optional-plugins list, collapsed at the bottom of the dashboard.
fn plugins_section(ui: &mut egui::Ui, browser: &BrowserManager) {
    egui::CollapsingHeader::new(format!("Plugins ({})", browser.plugins().len()))
        .default_open(false)
        .show(ui, |ui| {
            if browser.plugins().is_empty() {
                ui.weak("No plugins installed. Open the Browser tab to install one.");
                return;
            }
            for p in browser.plugins() {
                let (label, color) = if p.is_runnable() {
                    ("ready", egui::Color32::from_rgb(120, 200, 140))
                } else if !p.manifest.is_compatible() {
                    ("incompatible", ui.visuals().warn_fg_color)
                } else {
                    ("exe missing", egui::Color32::from_rgb(220, 120, 120))
                };
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                    ui.label(RichText::new(&p.manifest.name).strong())
                        .on_hover_text(p.dir.display().to_string());
                    ui.label(
                        RichText::new(format!("v{}", p.manifest.version))
                            .weak()
                            .small(),
                    );
                    ui.label(RichText::new(label).color(color).small());
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_orders_waiting_live_stuck_idle() {
        assert!(rank(InstanceStatus::WaitingForInput) < rank(InstanceStatus::Running));
        assert!(rank(InstanceStatus::Running) < rank(InstanceStatus::Paused));
        assert!(rank(InstanceStatus::Paused) <= rank(InstanceStatus::Dead));
        assert!(rank(InstanceStatus::Dead) < rank(InstanceStatus::Idle));
        assert_eq!(
            rank(InstanceStatus::Starting),
            rank(InstanceStatus::Thinking)
        );
    }

    #[test]
    fn role_emoji_known_and_fallback() {
        assert_eq!(role_emoji("reviewer"), "\u{1f50d}");
        assert_eq!(role_emoji("tester"), "\u{1f9ea}");
        assert_eq!(role_emoji("code-puppy"), "\u{1f415}");
        assert_eq!(role_emoji("anything-else"), "\u{1f415}");
    }

    #[test]
    fn card_state_vocabulary() {
        let a = Accents::from_palette(&crate::theme::ThemePalette::dark());
        let weak = Color32::GRAY;
        let s = card_state(InstanceStatus::ToolCalling, &a, weak);
        assert_eq!(s.label, "Digging");
        assert!(s.live);
        let s = card_state(InstanceStatus::WaitingForInput, &a, weak);
        assert_eq!(s.label, "Needs you");
        assert!(!s.live);
        let s = card_state(InstanceStatus::Dead, &a, weak);
        assert_eq!(s.label, "Stuck");
        let s = card_state(InstanceStatus::Paused, &a, weak);
        assert_eq!(s.label, "Napping");
    }

    #[test]
    fn tilde_path_abbreviates_home_only() {
        // A path that can't be under any home dir stays absolute.
        let p = std::path::Path::new("/definitely/not/home/proj");
        let s = tilde_path(p);
        assert!(s.ends_with("/definitely/not/home/proj") || s == "/definitely/not/home/proj");
    }
}
