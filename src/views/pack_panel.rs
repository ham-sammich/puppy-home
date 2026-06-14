//! The Den panel (internally still "pack", see [`crate::pack::DEN_LABEL`]):
//! join a relay room, see who's around and what their puppies are doing, and
//! chat. The full Den UI (roster/board/feed) lands on the redesign branches —
//! this panel renders the classic minimal view over the same [`DenState`].

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use puppy_relay::protocol::{ClaimInfo, FeedKind, MemberInfo, ServerMsg};
use serde_json::{Value, json};

use crate::pack::{DEN_LABEL, DenState, PackClient, PackEvent};

/// Keep the feed bounded (it's a live room, not an archive).
const FEED_CAP: usize = 500;

/// One line in the room feed.
enum FeedItem {
    Chat {
        from: String,
        text: String,
    },
    /// Presence notes ("bob joined") and errors.
    Note(String),
}

/// A live, joined connection.
struct Conn {
    client: PackClient,
    rx: Receiver<PackEvent>,
    /// The relay address we connected to (rides in the breadcrumb so the
    /// agent helper can reach the same relay).
    addr: String,
    room: String,
    members: Vec<MemberInfo>,
    /// user -> (kind, detail) of their latest activity ping.
    activity: HashMap<String, (String, String)>,
    /// Active file claims in the room (relay-broadcast on change).
    claims: Vec<ClaimInfo>,
    feed: Vec<FeedItem>,
    input: String,
    /// The full den mirror (members/roster/feed/kanban/plans) the redesign
    /// UI reads; folded from the same events as the classic view above.
    den: DenState,
}

/// State for the Pack tab (one instance, lives in the app).
pub struct PackView {
    pub relay: String,
    pub room: String,
    pub user: String,
    /// This member's puppy name (refreshed from the workspaces each frame).
    pub puppy: String,
    pub error: Option<String>,
    conn: Option<Conn>,
}

impl Default for PackView {
    fn default() -> Self {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "puppy".to_string());
        PackView {
            relay: "127.0.0.1:9220".to_string(),
            room: String::new(),
            user,
            puppy: String::new(),
            error: None,
            conn: None,
        }
    }
}

impl PackView {
    /// Is there a live room connection (used to gate activity broadcasts)?
    pub fn connected(&self) -> bool {
        self.conn.is_some()
    }

    /// The live den mirror, when joined (what the redesign UI renders).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn den(&self) -> Option<&DenState> {
        self.conn.as_ref().map(|c| &c.den)
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

    /// The `.puppy/pack.json` breadcrumb body the app drops in each workspace so
    /// every sidecar can inject "[pack context] ..." into prompts (Tier 2).
    /// `None` when not in a room. The app stamps `updated` at write time, so
    /// this stays change-comparable.
    pub fn breadcrumb(&self) -> Option<Value> {
        let conn = self.conn.as_ref()?;
        let members: Vec<Value> = conn
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
        let chat: Vec<Value> = conn
            .feed
            .iter()
            .filter_map(|item| match item {
                FeedItem::Chat { from, text } => Some(json!({ "from": from, "text": text })),
                FeedItem::Note(_) => None,
            })
            .collect();
        let recent: Vec<Value> = chat.iter().rev().take(10).rev().cloned().collect();
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

    /// Drain relay events into the view state.
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

/// Fold one relay message into the connection state: the den mirror first
/// (it understands every v4 event), then the classic view's bits.
fn apply(conn: &mut Conn, msg: ServerMsg) {
    conn.den.apply(&msg);
    match msg {
        ServerMsg::Joined { room, members, .. } => {
            conn.room = room;
            conn.members = members;
        }
        ServerMsg::MemberJoined { user, puppy, .. } => {
            let note = if puppy.is_empty() {
                format!("{user} joined")
            } else {
                format!("{user} joined with {puppy}")
            };
            if !conn.members.iter().any(|m| m.user == user) {
                conn.members.push(MemberInfo {
                    user,
                    puppy,
                    ..Default::default()
                });
                conn.members.sort_by(|a, b| a.user.cmp(&b.user));
            }
            push(conn, FeedItem::Note(note));
        }
        ServerMsg::MemberLeft { user } => {
            conn.members.retain(|m| m.user != user);
            conn.activity.remove(&user);
            push(conn, FeedItem::Note(format!("{user} left")));
        }
        ServerMsg::Feed { entry } => match entry.kind {
            FeedKind::Human => push(
                conn,
                FeedItem::Chat {
                    from: entry.user,
                    text: entry.text,
                },
            ),
            FeedKind::Puppy => {
                // "Rex → Biscuit:" when addressed, else just the puppy.
                let from = if entry.to_puppy.is_empty() {
                    format!("🐶 {}", entry.puppy)
                } else {
                    format!("🐶 {} → {}", entry.puppy, entry.to_puppy)
                };
                push(
                    conn,
                    FeedItem::Chat {
                        from,
                        text: entry.text,
                    },
                );
            }
            FeedKind::System => push(conn, FeedItem::Note(entry.text)),
        },
        ServerMsg::Activity {
            from, kind, detail, ..
        } => {
            conn.activity.insert(from, (kind, detail));
        }
        ServerMsg::Claims { items } => conn.claims = items,
        // Den structure events: the mirror above already folded them; the
        // classic panel has no surface for them yet (redesign branches do).
        ServerMsg::Presence { .. }
        | ServerMsg::Roster { .. }
        | ServerMsg::Tasks { .. }
        | ServerMsg::Plans { .. } => {}
        // Replies to coordination ops; the panel doesn't issue those (the
        // agent helper does, over its own one-shot connections).
        ServerMsg::ClaimResult { .. }
        | ServerMsg::ReleaseResult { .. }
        | ServerMsg::PostResult { .. } => {}
        ServerMsg::Error { message } => push(conn, FeedItem::Note(format!("relay: {message}"))),
    }
}

fn push(conn: &mut Conn, item: FeedItem) {
    conn.feed.push(item);
    if conn.feed.len() > FEED_CAP {
        let excess = conn.feed.len() - FEED_CAP;
        conn.feed.drain(..excess);
    }
}

/// Render the Pack tab. `puppy` is the local puppy's name (from the open
/// workspaces), attached to our presence + breadcrumb.
pub fn render(ui: &mut egui::Ui, view: &mut PackView, puppy: &str) {
    if !puppy.is_empty() {
        view.puppy = puppy.to_string();
    }
    view.poll();
    match &view.conn {
        None => render_join_form(ui, view),
        Some(_) => render_room(ui, view),
    }
}

fn render_join_form(ui: &mut egui::Ui, view: &mut PackView) {
    ui.add_space(8.0);
    ui.heading(format!("Puppy {DEN_LABEL}"));
    ui.label(format!(
        "Join a {DEN_LABEL} to see your teammates and chat. The room code is the shared secret.",
    ));
    ui.add_space(8.0);

    egui::Grid::new("pack-join-grid")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Relay (host[:port]):");
            ui.add(egui::TextEdit::singleline(&mut view.relay).desired_width(240.0));
            ui.end_row();
            ui.label("Room code:");
            ui.add(
                egui::TextEdit::singleline(&mut view.room)
                    .desired_width(240.0)
                    .hint_text("swift-otter-42"),
            );
            ui.end_row();
            ui.label("Your name:");
            ui.add(egui::TextEdit::singleline(&mut view.user).desired_width(240.0));
            ui.end_row();
        });

    if let Some(err) = &view.error {
        ui.add_space(4.0);
        ui.colored_label(ui.visuals().error_fg_color, err);
    }
    ui.add_space(8.0);

    let ready = !view.relay.trim().is_empty()
        && !view.room.trim().is_empty()
        && !view.user.trim().is_empty();
    if ui
        .add_enabled(ready, egui::Button::new(format!("Join {DEN_LABEL}")))
        .clicked()
    {
        match PackClient::connect(
            view.relay.trim(),
            view.room.trim(),
            view.user.trim(),
            view.puppy.trim(),
            "",
            "",
            crate::waker::egui_waker(ui.ctx()),
        ) {
            Ok((client, rx)) => {
                view.error = None;
                view.conn = Some(Conn {
                    client,
                    rx,
                    addr: view.relay.trim().to_string(),
                    room: view.room.trim().to_string(),
                    members: Vec::new(),
                    activity: HashMap::new(),
                    claims: Vec::new(),
                    feed: Vec::new(),
                    input: String::new(),
                    den: DenState::default(),
                });
            }
            Err(e) => view.error = Some(e),
        }
    }
    ui.add_space(4.0);
    ui.weak("Run a relay anywhere reachable:  puppy-relay [port]");
}

fn render_room(ui: &mut egui::Ui, view: &mut PackView) {
    let me = view.user.trim().to_string();
    let mut leave = false;
    let mut send: Option<String> = None;
    let conn = view.conn.as_mut().expect("checked by caller");

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{DEN_LABEL}: {}", conn.room)).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Leave").clicked() {
                leave = true;
            }
        });
    });
    ui.separator();

    // Members + their latest activity.
    ui.label(egui::RichText::new("MEMBERS").small().weak());
    for member in &conn.members {
        ui.horizontal(|ui| {
            let mut label = if member.user == me {
                format!("{} (you)", member.user)
            } else {
                member.user.clone()
            };
            if !member.puppy.is_empty() {
                label = format!("{label} \u{1f436} {}", member.puppy);
            }
            ui.label(label);
            if let Some((kind, detail)) = conn.activity.get(&member.user) {
                ui.weak(format!("- {kind}: {detail}"));
            }
        });
    }
    if !conn.claims.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("CLAIMS").small().weak());
        for c in &conn.claims {
            let who = if c.puppy.is_empty() {
                c.user.clone()
            } else {
                format!("{} ({})", c.user, c.puppy)
            };
            let line = if c.note.is_empty() {
                format!("{} - {who}", c.path)
            } else {
                format!("{} - {who}: {}", c.path, c.note)
            };
            ui.label(egui::RichText::new(line).monospace().small());
        }
    }
    ui.separator();

    // Feed (chat + presence notes), newest at the bottom.
    let row = ui.text_style_height(&egui::TextStyle::Body);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .max_height(ui.available_height() - row * 2.5)
        .id_salt("pack-feed")
        .show(ui, |ui| {
            for item in &conn.feed {
                match item {
                    FeedItem::Chat { from, text } => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(format!("{from}:")).strong());
                            ui.label(text);
                        });
                    }
                    FeedItem::Note(text) => {
                        ui.weak(text);
                    }
                }
            }
        });

    // Chat input.
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut conn.input)
                .desired_width(ui.available_width() - 70.0)
                .hint_text(format!("Message the {DEN_LABEL}…")),
        );
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Send").clicked() || enter) && !conn.input.trim().is_empty() {
            send = Some(conn.input.trim().to_string());
            conn.input.clear();
            field.request_focus();
        }
    });

    if let Some(text) = send {
        conn.client.chat(&text);
    }
    if leave {
        conn.client.leave();
        view.conn = None;
        view.error = None;
    }
}
