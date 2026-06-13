//! The Command Center dashboard, GPUI edition: pack header (puppy lede +
//! fleet stat tiles), attention banner, and the Grid / List / Focus fleet.
//!
//! Pattern (see GPUI_NOTES.md): `render` never hands live `&Workspace`s to
//! components. The root view builds plain [`CardSnapshot`]s once per frame
//! and feeds them to `RenderOnce` components ([`card::AgentCard`],
//! [`table::FleetTable`]); interaction flows back through
//! `Entity<RootView>::update → RootView::dispatch(DashAction)`.

pub mod card;
pub mod header;
pub mod model_pill;
pub mod table;

pub use header::{attention_banner, pack_header, segmented};

use std::time::Instant;

use gpui::{AnyElement, Entity, IntoElement, ParentElement as _, Rgba, Styled as _, div, px};

use crate::session::DashboardViewMode;
use crate::supervisor::Supervisor;
use crate::workspace::{InstanceStatus, Workspace, WorkspaceId};

use super::widgets;
use super::{RootView, Tokens};

/// Card grid: minimum card width before a row wraps (mock: minmax(420px,1fr)).
const CARD_MIN_W: f32 = 420.0;
/// Focus view: single column, capped.
const FOCUS_MAX_W: f32 = 880.0;

/// Which inline input a card has expanded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputKind {
    Steer,
    Send,
}

/// The one open inline card input (steer / new-prompt) — one at a time,
/// owned by the root view so it survives re-renders.
pub struct CardInput {
    pub ws: WorkspaceId,
    pub kind: InputKind,
    pub text: String,
    /// Steer delivery: false = now, true = queue.
    pub queue: bool,
}

/// Pack-vocabulary state derived from an [`InstanceStatus`].
pub struct CardState {
    pub label: &'static str,
    pub color: Rgba,
    pub live: bool,
}

/// Status → pack vocabulary (same table as redesign/egui's dashboard).
pub fn card_state(status: InstanceStatus, t: &Tokens) -> CardState {
    let (label, color, live) = match status {
        InstanceStatus::Starting => ("Waking up", t.weak, false),
        InstanceStatus::Running => ("Fetching", t.run, true),
        InstanceStatus::Thinking => ("Sniffing", t.think, true),
        InstanceStatus::ToolCalling => ("Digging", t.think, true),
        InstanceStatus::WaitingForInput => ("Needs you", t.wait, false),
        InstanceStatus::Paused => ("Napping", t.paused, false),
        InstanceStatus::Idle => ("Resting", t.dim, false),
        InstanceStatus::Dead => ("Stuck", t.error, false),
    };
    CardState { label, color, live }
}

/// Sort rank: needs-you first, then live, then paused/stuck, then resting.
///
/// `Starting` ranks with `Idle` (resting), not the live band: a cold spawn
/// has no work yet and settles to Idle, so banding it as "live" would make a
/// freshly opened card sit high then drop to the tail once ready (F7).
pub fn rank(status: InstanceStatus) -> u8 {
    match status {
        InstanceStatus::WaitingForInput => 0,
        InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => 1,
        InstanceStatus::Paused | InstanceStatus::Dead => 2,
        InstanceStatus::Idle | InstanceStatus::Starting => 3,
    }
}

/// The role emoji for an agent name (one puppy, many roles).
pub fn role_emoji(agent: &str) -> &'static str {
    match agent {
        "planner" => "\u{1f5fa}",   // map
        "reviewer" => "\u{1f50d}",  // magnifier
        "tester" => "\u{1f9ea}",    // test tube
        "docs" => "\u{1f4d6}",      // book
        "architect" => "\u{1f4d0}", // triangle ruler
        _ => "\u{1f415}",           // dog (code-puppy + unknown roles)
    }
}

/// Abbreviate the home dir to `~` (resolved once — render-hot path).
pub fn tilde_path(p: &std::path::Path) -> String {
    static HOME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| h.to_string_lossy().into_owned())
    });
    let s = p.display().to_string();
    match home {
        Some(h) if s.starts_with(h.as_str()) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

/// One sub-agent line, pre-resolved for rendering.
pub struct SubSnap {
    pub emoji: &'static str,
    pub name: String,
    pub model: String,
    pub status: String,
    pub color: Rgba,
    pub tool: Option<String>,
    pub tools: u64,
}

/// Everything a card or table row renders, detached from the live Workspace.
pub struct CardSnapshot {
    pub id: WorkspaceId,
    pub name: String,
    /// The workspace's puppy when it differs from the app headline (a
    /// remote host's puppy) — shown subtly on the meta line (B13.8).
    pub puppy: Option<String>,
    /// SSH-fallback workspace: local sidecar, ssh transport (meta-labeled).
    pub fallback: bool,
    pub agent: String,
    pub model: String,
    pub path: String,
    pub emoji: &'static str,
    pub status: InstanceStatus,
    pub label: &'static str,
    pub color: Rgba,
    pub live: bool,
    /// Idle/starting cards keep the neutral border (no status tint).
    pub neutral: bool,
    pub question: Option<String>,
    pub status_line: String,
    pub tool: Option<String>,
    /// Pre-formatted elapsed ("3:04") or last-seen ("4m ago") clock.
    pub clock: String,
    pub last_prompt: String,
    pub queued: u64,
    pub rate: f64,
    pub tokens: u64,
    pub tools: u64,
    pub adds: u64,
    pub dels: u64,
    /// Context-window fullness 0–100; `None` = unknown (no bar, no lie).
    pub ctx_pct: Option<f64>,
    pub cost: Option<f64>,
    /// `cost` came from the sidecar's dated models.dev snapshot (≈ marker).
    pub cost_estimated: bool,
    pub diff_count: usize,
    pub sparks: Vec<f32>,
    pub subs: Vec<SubSnap>,
    /// Model catalog `(name, description)` — filled only while this card's
    /// model popover is open (the only consumer).
    pub catalog: Option<Vec<(String, String)>>,
}

/// Map a sub-agent's free-text status onto the status palette.
fn sub_status_color(status: &str, t: &Tokens) -> Rgba {
    match status {
        "running" | "thinking" | "tool_calling" | "starting" => t.think,
        "completed" | "done" | "success" => t.run,
        "error" | "failed" | "cancelled" => t.error,
        _ => t.dim,
    }
}

/// Build a render snapshot of one workspace.
pub fn snapshot(
    ws: &Workspace,
    t: &Tokens,
    with_catalog: bool,
    headline_puppy: &str,
) -> CardSnapshot {
    let st = card_state(ws.status, t);
    let now = Instant::now();
    let clock = match ws.turn_started {
        Some(t0) => widgets::fmt_elapsed(now.saturating_duration_since(t0).as_secs()),
        None => widgets::fmt_ago(now.saturating_duration_since(ws.last_activity).as_secs()),
    };
    let (adds, dels) = ws.diff_totals();
    let puppy =
        (!ws.puppy_name.is_empty() && ws.puppy_name != "Puppy" && ws.puppy_name != headline_puppy)
            .then(|| ws.puppy_name.clone());
    CardSnapshot {
        id: ws.id,
        name: ws.name.clone(),
        puppy,
        fallback: ws.remote_fallback(),
        agent: if ws.agent.is_empty() {
            "agent".to_string()
        } else {
            ws.agent.clone()
        },
        model: if ws.model.is_empty() {
            "model\u{2026}".to_string()
        } else {
            ws.model.clone()
        },
        path: tilde_path(&ws.root),
        emoji: role_emoji(&ws.agent),
        status: ws.status,
        label: st.label,
        color: st.color,
        live: st.live,
        neutral: matches!(ws.status, InstanceStatus::Idle | InstanceStatus::Starting),
        question: ws.pending_question().map(str::to_string),
        status_line: ws.status_line.clone(),
        tool: ws.current_tool.clone(),
        clock,
        last_prompt: ws.last_prompt.clone(),
        queued: ws.queued_steers,
        rate: ws.token_rate,
        tokens: ws.total_tokens,
        tools: ws.tool_calls,
        adds,
        dels,
        ctx_pct: ws.ctx_pct,
        cost: ws.cost,
        cost_estimated: ws.cost_estimated,
        diff_count: ws.diff_count(),
        sparks: ws.spark_history().to_vec(),
        subs: ws
            .sub_agents
            .iter()
            .map(|sa| SubSnap {
                emoji: role_emoji(&sa.agent_name),
                name: sa.agent_name.clone(),
                model: sa.model_name.clone(),
                status: sa.status.clone(),
                color: sub_status_color(&sa.status, t),
                tool: sa.current_tool.clone(),
                tools: sa.tool_call_count,
            })
            .collect(),
        catalog: with_catalog.then(|| {
            ws.model_catalog()
                .iter()
                .map(|m| (m.name.clone(), m.description.clone()))
                .collect()
        }),
    }
}

/// Fleet counts for the lede + tiles.
pub struct FleetStats {
    pub dirs: usize,
    pub running: usize,
    pub napping: usize,
    pub waiting: usize,
    pub stuck: usize,
    pub tps: f64,
    pub tokens: u64,
    pub tools: u64,
    pub cost: Option<f64>,
    /// Any priced contribution was an estimate (≈ on the Spend tile).
    pub cost_estimated: bool,
}

pub fn fleet_stats(sup: &Supervisor) -> FleetStats {
    let mut s = FleetStats {
        dirs: sup.len(),
        running: 0,
        napping: 0,
        waiting: 0,
        stuck: 0,
        tps: 0.0,
        tokens: 0,
        tools: 0,
        cost: None,
        cost_estimated: false,
    };
    for ws in sup.iter() {
        match ws.status {
            InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => {
                s.running += 1;
                s.tps += ws.token_rate;
            }
            InstanceStatus::Paused => s.napping += 1,
            InstanceStatus::WaitingForInput => s.waiting += 1,
            InstanceStatus::Dead => s.stuck += 1,
            _ => {}
        }
        s.tokens += ws.total_tokens;
        s.tools += ws.tool_calls;
        if let Some(c) = ws.cost {
            *s.cost.get_or_insert(0.0) += c;
            s.cost_estimated |= ws.cost_estimated;
        }
    }
    s
}

/// The fleet body for the active view mode.
pub fn fleet(
    t: &Tokens,
    mode: DashboardViewMode,
    mut cards: Vec<CardSnapshot>,
    root: &Entity<RootView>,
    input: Option<(WorkspaceId, InputKind, String, bool)>,
    input_focus: &gpui::FocusHandle,
    reduce_motion: bool,
) -> AnyElement {
    // Reverse first so a STABLE sort keeps the newest workspace at the front
    // of its rank — a freshly spawned card lands top-left and stays, instead
    // of being appended to the tail and jumping bands as it boots (F7).
    cards.reverse();
    cards.sort_by_key(|c| rank(c.status));
    match mode {
        DashboardViewMode::List => table::FleetTable {
            t: *t,
            rows: cards,
            root: root.clone(),
        }
        .into_any_element(),
        DashboardViewMode::Grid => div()
            .flex()
            .flex_wrap()
            .gap_3p5()
            .children(cards.into_iter().map(|snap| {
                div().min_w(px(CARD_MIN_W)).flex_1().child(card_for(
                    t,
                    snap,
                    root,
                    &input,
                    input_focus,
                    reduce_motion,
                ))
            }))
            .into_any_element(),
        DashboardViewMode::Focus => div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3p5()
            .children(cards.into_iter().map(|snap| {
                div().w_full().max_w(px(FOCUS_MAX_W)).child(card_for(
                    t,
                    snap,
                    root,
                    &input,
                    input_focus,
                    reduce_motion,
                ))
            }))
            .into_any_element(),
    }
}

fn card_for(
    t: &Tokens,
    snap: CardSnapshot,
    root: &Entity<RootView>,
    input: &Option<(WorkspaceId, InputKind, String, bool)>,
    input_focus: &gpui::FocusHandle,
    reduce_motion: bool,
) -> card::AgentCard {
    let inline = input
        .as_ref()
        .filter(|(id, ..)| *id == snap.id)
        .map(|(_, kind, text, queue)| (*kind, text.clone(), *queue));
    card::AgentCard {
        t: *t,
        snap,
        root: root.clone(),
        inline,
        input_focus: input_focus.clone(),
        reduce_motion,
    }
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
    }

    #[test]
    fn starting_ranks_with_idle_not_live() {
        // F7: a cold spawn must not band as "live" (it would jump to the tail
        // once it settles to Idle). Starting and Idle share the resting rank.
        assert_eq!(rank(InstanceStatus::Starting), rank(InstanceStatus::Idle));
        assert!(rank(InstanceStatus::Running) < rank(InstanceStatus::Starting));
    }

    #[test]
    fn card_state_pack_vocabulary() {
        let t = Tokens::dark();
        let s = card_state(InstanceStatus::ToolCalling, &t);
        assert_eq!(s.label, "Digging");
        assert!(s.live);
        let s = card_state(InstanceStatus::WaitingForInput, &t);
        assert_eq!(s.label, "Needs you");
        assert!(!s.live);
        let s = card_state(InstanceStatus::Paused, &t);
        assert_eq!(s.label, "Napping");
        let s = card_state(InstanceStatus::Dead, &t);
        assert_eq!(s.label, "Stuck");
        let s = card_state(InstanceStatus::Idle, &t);
        assert_eq!(s.label, "Resting");
    }

    #[test]
    fn role_emoji_known_and_fallback() {
        assert_eq!(role_emoji("reviewer"), "\u{1f50d}");
        assert_eq!(role_emoji("code-puppy"), "\u{1f415}");
        assert_eq!(role_emoji("mystery"), "\u{1f415}");
    }

    #[test]
    fn tilde_path_abbreviates_home_only() {
        let p = std::path::Path::new("/definitely/not/home/proj");
        let s = tilde_path(p);
        assert!(s.ends_with("/definitely/not/home/proj"));
    }
}
