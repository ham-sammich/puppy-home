//! The Den: the collaborative room UI over the Phase-0 Den protocol v4.
//! Disconnected = a restyled join form; joined = room header + den-work
//! (Roster / Board, see `roster.rs` / `board.rs`) + the coordination feed
//! (`feed.rs`). Internal names keep "pack" (see [`crate::pack::DEN_LABEL`]);
//! everything a user reads says Den.

mod board;
mod feed;
mod join;
mod roster;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontFamily, RichText};
use puppy_relay::protocol::{ClaimInfo, MemberInfo, Presence, ServerMsg};
use serde_json::{Value, json};

use crate::fonts::FAMILY_GROTESK_BOLD;
use crate::pack::{DEN_LABEL, DenState, PackClient, PackEvent};
use crate::shell::ShellAction;
use crate::supervisor::Supervisor;
use crate::theme::Accents;
use crate::views::widgets::{self, Toasts};
use crate::workspace::SparkRing;

/// No input for this long (or an unfocused window) = idle presence.
const IDLE_AFTER: Duration = Duration::from_secs(300);
/// tok/s snapshots kept per remote agent (one per roster broadcast ≈ 2.5s).
const REMOTE_SPARK_SAMPLES: usize = 20;

/// Presence heuristic: active only while the window is focused AND the user
/// touched mouse/keyboard within [`IDLE_AFTER`]. Pure for testing.
fn presence_for(focused: bool, since_input: Duration) -> Presence {
    if focused && since_input < IDLE_AFTER {
        Presence::Active
    } else {
        Presence::Idle
    }
}

/// Which half of den-work is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkView {
    Roster,
    Board,
}

/// Board-local UI state (inline add-card and retitle inputs).
#[derive(Default)]
struct BoardUi {
    /// An open "+ add card" input: (column, draft title).
    add: Option<(puppy_relay::protocol::TaskColumn, String)>,
    /// An open retitle input: (task id, draft title).
    retitle: Option<(u64, String)>,
}

/// A live, joined connection + everything folded from it.
struct Conn {
    client: PackClient,
    rx: Receiver<PackEvent>,
    /// The relay address we connected to (header + invite + breadcrumb).
    addr: String,
    room: String,
    /// user -> (kind, detail) of their latest activity ping (breadcrumb).
    activity: HashMap<String, (String, String)>,
    /// Active file claims (breadcrumb + a collapsed roster section).
    claims: Vec<ClaimInfo>,
    /// The den mirror (members/roster/feed/kanban/plans).
    den: DenState,
    /// The feed composer draft.
    input: String,
    work: WorkView,
    /// Feed tail-cap override ("Show older").
    show_all_feed: bool,
    /// user -> dir -> short tok/s history, derived from successive roster
    /// snapshots (bounded: pruned per roster update + on member leave).
    sparks: HashMap<String, HashMap<String, SparkRing>>,
    /// Last roster `ts` folded per user (skip duplicate snapshots).
    spark_seen: HashMap<String, u64>,
    /// Last presence we told the relay (send only on change).
    presence_sent: Presence,
    board: BoardUi,
}

/// State for the Den tab (one instance, lives in the app).
pub struct DenView {
    pub relay: String,
    pub room: String,
    pub user: String,
    /// This member's puppy name (refreshed from the workspaces each frame).
    pub puppy: String,
    pub error: Option<String>,
    conn: Option<Conn>,
    toasts: Toasts,
    /// Last local mouse/keyboard activity (the presence heuristic).
    last_input_at: Instant,
}

impl Default for DenView {
    fn default() -> Self {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "puppy".to_string());
        DenView {
            relay: "127.0.0.1:9220".to_string(),
            room: String::new(),
            user,
            puppy: String::new(),
            error: None,
            conn: None,
            toasts: Toasts::default(),
            last_input_at: Instant::now(),
        }
    }
}

impl DenView {
    /// Is there a live room connection (used to gate activity broadcasts)?
    pub fn connected(&self) -> bool {
        self.conn.is_some()
    }

    /// Broadcast this member's roster agent summaries, if connected.
    pub fn send_roster(&self, agents: Vec<puppy_relay::protocol::RoomAgentInfo>) {
        if let Some(conn) = &self.conn {
            conn.client.send_roster(agents);
        }
    }

    /// Broadcast an activity ping to the room, if connected.
    pub fn send_activity(&self, kind: &str, detail: &str) {
        if let Some(conn) = &self.conn {
            conn.client.activity(kind, detail);
        }
    }

    /// Presence heuristic, run every frame by the app (Den tab visible or
    /// not): idle = window unfocused OR no mouse/keyboard for 5 minutes;
    /// sent to the relay only when the value changes.
    pub fn tick_presence(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if ctx.input(|i| !i.events.is_empty() || i.pointer.any_down()) {
            self.last_input_at = Instant::now();
        }
        let since = self.last_input_at.elapsed();
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let presence = presence_for(focused, since);
        if presence != conn.presence_sent {
            conn.client.send_presence(presence);
            conn.presence_sent = presence;
        }
    }

    /// The `.puppy/pack.json` breadcrumb body the app drops in each workspace
    /// so every sidecar can inject "[pack context] ..." into prompts (Tier 2).
    /// `None` when not in a room.
    pub fn breadcrumb(&self) -> Option<Value> {
        let conn = self.conn.as_ref()?;
        let members: Vec<Value> = conn
            .den
            .members
            .iter()
            .map(|m| {
                let activity = conn
                    .activity
                    .get(&m.user)
                    .map(|(kind, detail)| {
                        if kind == "status" {
                            detail.clone()
                        } else {
                            format!("{kind}: {detail}")
                        }
                    })
                    .unwrap_or_default();
                json!({ "user": m.user, "puppy": m.puppy, "activity": activity })
            })
            .collect();
        // Most recent 10 conversational entries (humans + puppies).
        let mut recent: Vec<Value> = conn
            .den
            .feed
            .iter()
            .rev()
            .filter_map(|e| match e.kind {
                puppy_relay::protocol::FeedKind::Human => {
                    Some(json!({ "from": e.user, "text": e.text }))
                }
                puppy_relay::protocol::FeedKind::Puppy => {
                    Some(json!({ "from": format!("\u{1f436} {}", e.puppy), "text": e.text }))
                }
                puppy_relay::protocol::FeedKind::System => None,
            })
            .take(10)
            .collect();
        recent.reverse();
        let claims: Vec<Value> = conn
            .claims
            .iter()
            .map(|c| {
                json!({
                    "path": c.path, "user": c.user, "puppy": c.puppy, "note": c.note,
                })
            })
            .collect();
        Some(json!({
            "room": conn.room,
            "relay": conn.addr,
            "user": self.user.trim(),
            "puppy": self.puppy.trim(),
            "members": members,
            "claims": claims,
            "chat": recent,
        }))
    }

    /// Drain relay events into the den mirror (+ spark rings, claims).
    pub(crate) fn poll(&mut self) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let mut disconnected = false;
        loop {
            match conn.rx.try_recv() {
                Ok(PackEvent::Msg(msg)) => apply(conn, msg),
                Ok(PackEvent::Disconnected) | Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if disconnected {
            self.conn = None;
            self.error = Some("Disconnected from the relay.".to_string());
        }
    }
}

/// Fold one relay message: the den mirror first (it understands every v4
/// event), then the local extras (claims, activity, remote spark rings).
fn apply(conn: &mut Conn, msg: ServerMsg) {
    conn.den.apply(&msg);
    match msg {
        ServerMsg::Joined { room, .. } => conn.room = room,
        ServerMsg::Roster { from, agents, ts } => {
            // One sample per distinct broadcast: tok/s history for sparklines.
            if conn.spark_seen.get(&from) != Some(&ts) {
                conn.spark_seen.insert(from.clone(), ts);
                let rings = conn.sparks.entry(from).or_default();
                rings.retain(|dir, _| agents.iter().any(|a| &a.dir == dir));
                for a in &agents {
                    rings
                        .entry(a.dir.clone())
                        .or_insert_with(|| SparkRing::new(REMOTE_SPARK_SAMPLES))
                        .push(a.tps);
                }
            }
        }
        ServerMsg::MemberLeft { user } => {
            conn.activity.remove(&user);
            conn.sparks.remove(&user);
            conn.spark_seen.remove(&user);
        }
        ServerMsg::Activity {
            from, kind, detail, ..
        } => {
            conn.activity.insert(from, (kind, detail));
        }
        ServerMsg::Claims { items } => conn.claims = items,
        _ => {}
    }
}

/// A member's relay-assigned owner color, parsed (None = unknown user).
fn member_color(members: &[MemberInfo], user: &str) -> Option<Color32> {
    members
        .iter()
        .find(|m| m.user == user)
        .and_then(|m| crate::theme::parse_hex(&m.color))
}

/// The shared project: the most common workspace dir across all rosters.
fn project_name(den: &DenState) -> Option<&str> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (agents, _) in den.roster.values() {
        for a in agents {
            if !a.dir.is_empty() {
                *counts.entry(a.dir.as_str()).or_default() += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(d, _)| d)
}

/// Render the Den tab. `puppy` is the local puppy's name (from the open
/// workspaces), attached to our presence + nudges + breadcrumb.
pub fn render(
    ui: &mut egui::Ui,
    view: &mut DenView,
    sup: &Supervisor,
    accents: &Accents,
    actions: &mut Vec<ShellAction>,
    puppy: &str,
) {
    if !puppy.is_empty() {
        view.puppy = puppy.to_string();
    }
    view.poll();
    if view.conn.is_some() {
        render_room(ui, view, sup, accents, actions);
    } else {
        join::render_join_form(ui, view);
    }
    view.toasts.render(ui.ctx());
}

/// The joined room: header, then feed (right, 340px) + den-work (rest).
fn render_room(
    ui: &mut egui::Ui,
    view: &mut DenView,
    sup: &Supervisor,
    accents: &Accents,
    actions: &mut Vec<ShellAction>,
) {
    let me = view.user.trim().to_string();
    let my_puppy = view.puppy.trim().to_string();
    let toasts = &mut view.toasts;
    let Some(conn) = view.conn.as_mut() else {
        return;
    };
    let mut leave = false;

    render_header(ui, conn, accents, toasts, &mut leave);
    ui.separator();

    egui::Panel::right(egui::Id::new("den-feed-panel"))
        .resizable(false)
        .exact_size(340.0)
        .show_inside(ui, |ui| {
            feed::render(ui, conn, accents, &me);
        });

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("den-work-scroll")
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (label, hint) = match conn.work {
                    WorkView::Roster => ("Shared work", "everyone's puppy, on the same project"),
                    WorkView::Board => ("Project board", "tasks + shared plans"),
                };
                ui.label(RichText::new(label).strong());
                ui.label(RichText::new(hint).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::segmented(
                        ui,
                        &mut conn.work,
                        &[(WorkView::Roster, "Roster"), (WorkView::Board, "Board")],
                    );
                });
            });
            ui.add_space(6.0);
            match conn.work {
                WorkView::Roster => {
                    roster::render(ui, conn, sup, accents, actions, toasts, &me, &my_puppy)
                }
                WorkView::Board => board::render(ui, conn, sup, accents, toasts, &me, &my_puppy),
            }
        });

    if leave {
        conn.client.leave();
    }
    if leave {
        view.conn = None;
        view.error = None;
    }
}

/// Room header: LIVE · "Den · code" (click = copy) · people/puppies subtitle;
/// right: relay addr · + Invite · Leave den.
fn render_header(
    ui: &mut egui::Ui,
    conn: &mut Conn,
    accents: &Accents,
    toasts: &mut Toasts,
    leave: &mut bool,
) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        live_badge(ui, accents);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("\u{1f43e} {DEN_LABEL} \u{00b7}"))
                        .family(FontFamily::Name(FAMILY_GROTESK_BOLD.into()))
                        .size(16.0),
                );
                let code = widgets::pill(ui, RichText::new(&conn.room).monospace().size(12.0))
                    .on_hover_text("Copy the room code");
                if code.clicked() {
                    ui.ctx().copy_text(conn.room.clone());
                    toasts.push("Room code copied", accents.accent);
                }
            });
            let people = conn.den.members.len();
            let puppies = conn
                .den
                .members
                .iter()
                .filter(|m| !m.puppy.is_empty())
                .count();
            let mut sub = format!(
                "{people} {} \u{00b7} {puppies} {}",
                if people == 1 { "person" } else { "people" },
                if puppies == 1 { "puppy" } else { "puppies" },
            );
            if let Some(project) = project_name(&conn.den) {
                sub.push_str(&format!(" \u{00b7} working {project} together"));
            }
            ui.label(RichText::new(sub).weak().small());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(
                    RichText::new(format!("Leave {}", DEN_LABEL.to_lowercase()))
                        .color(accents.error),
                )
                .clicked()
            {
                *leave = true;
            }
            if ui
                .button("\u{ff0b} Invite")
                .on_hover_text("Copy a shareable invite line")
                .clicked()
            {
                ui.ctx().copy_text(format!(
                    "Join my {DEN_LABEL}: room {} \u{00b7} relay {}",
                    conn.room, conn.addr
                ));
                toasts.push("Invite copied to clipboard", accents.accent);
            }
            ui.label(
                RichText::new(format!("relay {}", conn.addr))
                    .monospace()
                    .weak()
                    .small(),
            );
        });
    });
    ui.add_space(6.0);
}

/// The blinking LIVE indicator (1.6s cycle), gated on motion_enabled; reduced
/// motion / unfocused renders a steady dot with no scheduled repaints.
fn live_badge(ui: &mut egui::Ui, accents: &Accents) {
    let animate = widgets::motion_enabled(ui.ctx());
    let alpha = if animate {
        let t = ui.input(|i| i.time);
        0.45 + 0.55 * ((t * std::f64::consts::TAU / 1.6).sin().abs() as f32)
    } else {
        1.0
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 4.0, accents.run.linear_multiply(alpha));
    ui.label(
        RichText::new("LIVE")
            .color(accents.run)
            .strong()
            .small()
            .monospace(),
    );
    if animate {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_needs_focus_and_recent_input() {
        let recent = Duration::from_secs(10);
        let stale = IDLE_AFTER + Duration::from_secs(1);
        assert_eq!(presence_for(true, recent), Presence::Active);
        assert_eq!(presence_for(false, recent), Presence::Idle);
        assert_eq!(presence_for(true, stale), Presence::Idle);
        assert_eq!(presence_for(false, stale), Presence::Idle);
        // Exactly at the boundary counts as idle (strictly-less-than window).
        assert_eq!(presence_for(true, IDLE_AFTER), Presence::Idle);
    }

    #[test]
    fn member_color_parses_relay_hex() {
        let members = vec![MemberInfo {
            user: "jordan".into(),
            puppy: "Biscuit".into(),
            color: "#56c7c2".into(),
            host: false,
            presence: Presence::Active,
        }];
        assert_eq!(
            member_color(&members, "jordan"),
            Some(Color32::from_rgb(0x56, 0xc7, 0xc2))
        );
        assert_eq!(member_color(&members, "nobody"), None);
    }
}
