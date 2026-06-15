//! Room membership + broadcast. Pure logic over `mpsc` senders (one per
//! connection's outbound queue), so it unit-tests without any sockets.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{
    ClaimInfo, FeedEntry, FeedKind, MemberInfo, PlanInfo, Presence, RoomAgentInfo, ServerMsg,
    TaskColumn, TaskInfo,
};

/// Claims auto-expire after this long (re-claiming refreshes the stamp), so a
/// crashed agent can't squat on a file forever.
const CLAIM_TTL_SECS: u64 = 3600;

/// How much feed history a room keeps (and hands to late joiners).
const FEED_TAIL_CAP: usize = 200;

/// Hard cap on one member's roster broadcast (bounded relay fan-out).
const ROSTER_CAP: usize = 16;

/// Owner colors, assigned round-robin by join order — unique per room until
/// it outgrows the palette, and nobody argues about favorites.
const OWNER_COLORS: [&str; 8] = [
    "#e7ab4d", "#56c7c2", "#b79cff", "#e58fb0", "#8fd17a", "#6aa8ff", "#f0986a", "#d9d96a",
];

/// All rooms + who's in them. One per relay process, shared across connections.
#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    /// room code -> live room state (dropped when the last member leaves --
    /// claims die with the room).
    rooms: HashMap<String, Room>,
    /// connection id -> room code (for leave/broadcast lookups)
    conns: HashMap<u64, String>,
}

#[derive(Default)]
struct Room {
    members: HashMap<u64, Member>,
    /// path -> active claim.
    claims: HashMap<String, Claim>,
    /// Bounded feed tail (also the late-joiner snapshot).
    feed: Vec<FeedEntry>,
    /// Room-monotonic feed ordering.
    feed_seq: u64,
    /// Kanban cards, in creation order. The relay is the source of truth.
    tasks: Vec<TaskInfo>,
    next_task_id: u64,
    /// Shared plans, keyed by (user, puppy) in share order.
    plans: Vec<PlanInfo>,
    /// Last roster agent-summary each member broadcast, keyed by user. Cached
    /// so late joiners see existing members' agents (the client only sends its
    /// roster when it CHANGES, so without this a late joiner would show
    /// "no agents reported yet" for everyone already in the room).
    rosters: HashMap<String, Vec<RoomAgentInfo>>,
    /// Monotonic join counter (smallest current value = host).
    join_seq: u64,
}

impl Room {
    fn broadcast(&self, line: &str) {
        for m in self.members.values() {
            let _ = m.tx.send(line.to_string());
        }
    }

    /// Append a feed entry (stamping seq + ts), trim the tail, broadcast it.
    fn push_feed(&mut self, kind: FeedKind, user: &str, puppy: &str, entry: FeedSeed) {
        self.feed_seq += 1;
        let item = FeedEntry {
            seq: self.feed_seq,
            kind,
            user: user.to_string(),
            puppy: puppy.to_string(),
            to_puppy: entry.to_puppy,
            review: entry.review,
            text: entry.text,
            ts: now_ts(),
        };
        self.feed.push(item.clone());
        if self.feed.len() > FEED_TAIL_CAP {
            let excess = self.feed.len() - FEED_TAIL_CAP;
            self.feed.drain(..excess);
        }
        self.broadcast(&encode(&ServerMsg::Feed { entry: item }));
    }

    /// A system narration line in the feed.
    fn system_feed(&mut self, text: String) {
        self.push_feed(FeedKind::System, "", "", FeedSeed::text(text));
    }

    /// Current members, host flag derived from the earliest join still present.
    fn member_infos(&self) -> Vec<MemberInfo> {
        let host_seq = self.members.values().map(|m| m.join_seq).min();
        let mut roster: Vec<MemberInfo> = self
            .members
            .values()
            .map(|m| MemberInfo {
                user: m.user.clone(),
                puppy: m.puppy.clone(),
                user_avatar: m.user_avatar.clone(),
                puppy_avatar: m.puppy_avatar.clone(),
                user_avatar_png: m.user_avatar_png.clone(),
                puppy_avatar_png: m.puppy_avatar_png.clone(),
                color: m.color.clone(),
                host: Some(m.join_seq) == host_seq,
                presence: m.presence,
            })
            .collect();
        roster.sort_by(|a, b| a.user.cmp(&b.user));
        roster
    }

    /// Broadcast the full kanban (the snapshot IS the delta at den scale).
    fn broadcast_tasks(&self) {
        self.broadcast(&encode(&ServerMsg::Tasks {
            items: self.tasks.clone(),
        }));
    }

    /// Broadcast every shared plan.
    fn broadcast_plans(&self) {
        self.broadcast(&encode(&ServerMsg::Plans {
            items: self.plans.clone(),
        }));
    }

    /// Drop expired claims, then snapshot the rest sorted by path.
    fn claims_snapshot(&mut self) -> Vec<ClaimInfo> {
        let now = now_ts();
        self.claims
            .retain(|_, c| now.saturating_sub(c.ts) < CLAIM_TTL_SECS);
        let mut items: Vec<ClaimInfo> = self
            .claims
            .iter()
            .map(|(path, c)| ClaimInfo {
                path: path.clone(),
                user: c.user.clone(),
                puppy: c.puppy.clone(),
                note: c.note.clone(),
                ts: c.ts,
            })
            .collect();
        items.sort_by(|a, b| a.path.cmp(&b.path));
        items
    }
}

struct Member {
    user: String,
    puppy: String,
    user_avatar: String,
    puppy_avatar: String,
    user_avatar_png: String,
    puppy_avatar_png: String,
    color: String,
    presence: Presence,
    /// Position in the room's join order (host = smallest present).
    join_seq: u64,
    tx: Sender<String>,
}

/// The avatars a member supplies at join: emoji (always renderable everywhere)
/// plus optional base64 PNG thumbnails when they chose a PHOTO pfp.
#[derive(Default, Clone)]
pub struct JoinAvatars {
    pub user_emoji: String,
    pub puppy_emoji: String,
    pub user_png: String,
    pub puppy_png: String,
}

/// The caller-supplied half of a feed entry (the room stamps the rest).
struct FeedSeed {
    to_puppy: String,
    review: bool,
    text: String,
}

impl FeedSeed {
    fn text(text: String) -> Self {
        FeedSeed {
            to_puppy: String::new(),
            review: false,
            text,
        }
    }
}

struct Claim {
    user: String,
    puppy: String,
    note: String,
    ts: u64,
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn encode(msg: &ServerMsg) -> String {
    serde_json::to_string(msg).expect("ServerMsg always serializes")
}

impl Hub {
    pub fn new() -> Self {
        Hub::default()
    }

    /// Add a member to a room (creating it on first join). Existing members
    /// are told (announcement + system feed line); the joiner gets the full
    /// den snapshot back: members, feed tail, kanban, shared plans.
    pub fn join(
        &self,
        room: &str,
        user: &str,
        puppy: &str,
        av: JoinAvatars,
        tx: Sender<String>,
    ) -> (u64, ServerMsg) {
        let mut inner = self.inner.lock().unwrap();
        inner.next_id += 1;
        let id = inner.next_id;

        let room_state = inner.rooms.entry(room.to_string()).or_default();
        room_state.join_seq += 1;
        let join_seq = room_state.join_seq;
        let color = OWNER_COLORS[(join_seq as usize - 1) % OWNER_COLORS.len()].to_string();

        room_state.broadcast(&encode(&ServerMsg::MemberJoined {
            user: user.into(),
            puppy: puppy.into(),
            user_avatar: av.user_emoji.clone(),
            puppy_avatar: av.puppy_emoji.clone(),
            user_avatar_png: av.user_png.clone(),
            puppy_avatar_png: av.puppy_png.clone(),
            color: color.clone(),
        }));
        room_state.system_feed(if puppy.is_empty() {
            format!("{user} joined the den")
        } else {
            format!("{user} joined the den with {puppy}")
        });
        room_state.members.insert(
            id,
            Member {
                user: user.to_string(),
                puppy: puppy.to_string(),
                user_avatar: av.user_emoji,
                puppy_avatar: av.puppy_emoji,
                user_avatar_png: av.user_png,
                puppy_avatar_png: av.puppy_png,
                color,
                presence: Presence::Active,
                join_seq,
                tx,
            },
        );
        let snapshot = ServerMsg::Joined {
            room: room.to_string(),
            members: room_state.member_infos(),
            feed: room_state.feed.clone(),
            tasks: room_state.tasks.clone(),
            plans: room_state.plans.clone(),
        };
        inner.conns.insert(id, room.to_string());
        (id, snapshot)
    }

    /// Replay every existing member's last-known roster to ONE member (the new
    /// joiner). MUST be called AFTER their `Joined` snapshot is sent, because
    /// the client clears its roster on `Joined` -- replaying earlier would get
    /// wiped. Clients only re-send their roster on change, so without this a
    /// late joiner shows "no agents reported yet" for everyone already here.
    pub fn replay_rosters(&self, id: u64) {
        let inner = self.inner.lock().unwrap();
        let Some(room) = inner.conns.get(&id) else {
            return;
        };
        let Some(room_state) = inner.rooms.get(room) else {
            return;
        };
        let Some(member) = room_state.members.get(&id) else {
            return;
        };
        for (from, agents) in &room_state.rosters {
            let _ = member.tx.send(encode(&ServerMsg::Roster {
                from: from.clone(),
                agents: agents.clone(),
                ts: now_ts(),
            }));
        }
    }

    /// Remove a member (disconnect or explicit leave); tells the room and drops
    /// the room entirely once it's empty.
    pub fn leave(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let Some(room) = inner.conns.remove(&id) else {
            return;
        };
        let user = inner
            .rooms
            .get_mut(&room)
            .and_then(|r| r.members.remove(&id))
            .map(|m| m.user);
        if let Some(user) = user
            && let Some(room_state) = inner.rooms.get_mut(&room)
        {
            // Drop the departed user's cached roster once none of their
            // connections remain (bounded memory; no stale agents replayed).
            if !room_state.members.values().any(|m| m.user == user) {
                room_state.rosters.remove(&user);
            }
            room_state.broadcast(&encode(&ServerMsg::MemberLeft { user: user.clone() }));
            room_state.system_feed(format!("{user} left the den"));
        }
        if inner.rooms.get(&room).is_some_and(|r| r.members.is_empty()) {
            inner.rooms.remove(&room);
        }
    }

    /// A human chat line -> a `Human` feed entry (sender included in the
    /// broadcast, so everyone shares the relay's ordering + timestamp).
    pub fn chat(&self, id: u64, text: &str) {
        self.room_of(id, |room, member| {
            let user = member.to_string();
            room.push_feed(FeedKind::Human, &user, "", FeedSeed::text(text.to_string()));
        });
    }

    /// A puppy speaking into the feed, optionally at another puppy.
    pub fn puppy_msg(&self, id: u64, puppy: &str, to_puppy: &str, review: bool, text: &str) {
        self.room_of(id, |room, member| {
            let user = member.to_string();
            room.push_feed(
                FeedKind::Puppy,
                &user,
                puppy,
                FeedSeed {
                    to_puppy: to_puppy.to_string(),
                    review,
                    text: text.to_string(),
                },
            );
        });
    }

    /// Update a member's presence and tell the room.
    pub fn presence(&self, id: u64, presence: Presence) {
        let mut inner = self.inner.lock().unwrap();
        let Some(room) = inner.conns.get(&id).cloned() else {
            return;
        };
        let Some(room_state) = inner.rooms.get_mut(&room) else {
            return;
        };
        let Some(member) = room_state.members.get_mut(&id) else {
            return;
        };
        if member.presence == presence {
            return; // no-op flaps stay off the wire
        }
        member.presence = presence;
        let user = member.user.clone();
        room_state.broadcast(&encode(&ServerMsg::Presence { user, presence }));
    }

    /// Re-broadcast a member's roster agent summaries (size-capped) AND cache
    /// them so the next late joiner gets them in their replay.
    pub fn roster(&self, id: u64, mut agents: Vec<RoomAgentInfo>) {
        agents.truncate(ROSTER_CAP);
        self.room_of(id, |room, member| {
            let user = member.to_string();
            room.rosters.insert(user.clone(), agents.clone());
            room.broadcast(&encode(&ServerMsg::Roster {
                from: user,
                agents,
                ts: now_ts(),
            }));
        });
    }

    // -- Kanban: the relay owns ids + ordering; every change re-broadcasts the
    // -- full board (small enough that the snapshot is the delta).

    pub fn task_create(&self, id: u64, title: &str, column: TaskColumn, owner: &str, plan: bool) {
        self.room_of(id, |room, _| {
            room.next_task_id += 1;
            room.tasks.push(TaskInfo {
                id: room.next_task_id,
                title: title.to_string(),
                column,
                owner: owner.to_string(),
                plan,
            });
            room.broadcast_tasks();
        });
    }

    pub fn task_move(&self, id: u64, task: u64, column: TaskColumn) {
        self.room_of(id, |room, _| {
            if let Some(t) = room.tasks.iter_mut().find(|t| t.id == task) {
                t.column = column;
                room.broadcast_tasks();
            }
        });
    }

    pub fn task_assign(&self, id: u64, task: u64, owner: &str) {
        self.room_of(id, |room, _| {
            if let Some(t) = room.tasks.iter_mut().find(|t| t.id == task) {
                t.owner = owner.to_string();
                room.broadcast_tasks();
            }
        });
    }

    pub fn task_retitle(&self, id: u64, task: u64, title: &str) {
        self.room_of(id, |room, _| {
            if let Some(t) = room.tasks.iter_mut().find(|t| t.id == task) {
                t.title = title.to_string();
                room.broadcast_tasks();
            }
        });
    }

    pub fn task_delete(&self, id: u64, task: u64) {
        self.room_of(id, |room, _| {
            let before = room.tasks.len();
            room.tasks.retain(|t| t.id != task);
            if room.tasks.len() != before {
                room.broadcast_tasks();
            }
        });
    }

    /// Share (or update) a puppy's plans.md; narrated in the feed.
    pub fn plan_share(&self, id: u64, puppy: &str, markdown: &str) {
        self.room_of(id, |room, member| {
            let user = member.to_string();
            let plan = PlanInfo {
                user: user.clone(),
                puppy: puppy.to_string(),
                markdown: markdown.to_string(),
                ts: now_ts(),
            };
            match room
                .plans
                .iter_mut()
                .find(|p| p.user == user && p.puppy == puppy)
            {
                Some(slot) => *slot = plan,
                None => room.plans.push(plan),
            }
            room.broadcast_plans();
            room.system_feed(format!("{puppy} shared plans.md"));
        });
    }

    /// Withdraw a shared plan; narrated in the feed.
    pub fn plan_unshare(&self, id: u64, puppy: &str) {
        self.room_of(id, |room, member| {
            let user = member.to_string();
            let before = room.plans.len();
            room.plans.retain(|p| !(p.user == user && p.puppy == puppy));
            if room.plans.len() != before {
                room.broadcast_plans();
                room.system_feed(format!("{puppy} withdrew plans.md"));
            }
        });
    }

    /// Run `act` with the sender's room + their user name (mutable room access
    /// — the shared spine of every den op).
    fn room_of(&self, id: u64, act: impl FnOnce(&mut Room, &str)) {
        let mut inner = self.inner.lock().unwrap();
        let Some(room) = inner.conns.get(&id).cloned() else {
            return;
        };
        let Some(room_state) = inner.rooms.get_mut(&room) else {
            return;
        };
        let Some(member) = room_state.members.get(&id) else {
            return;
        };
        let user = member.user.clone();
        act(room_state, &user);
    }

    /// Re-broadcast an activity ping to the whole room.
    pub fn activity(&self, id: u64, kind: &str, detail: &str) {
        self.broadcast_from(id, |user| ServerMsg::Activity {
            from: user.to_string(),
            kind: kind.to_string(),
            detail: detail.to_string(),
            ts: now_ts(),
        });
    }

    /// Build a message stamped with the sender's (relay-known) name and fan it
    /// out to every member of the sender's room.
    fn broadcast_from(&self, id: u64, build: impl FnOnce(&str) -> ServerMsg) {
        let inner = self.inner.lock().unwrap();
        let Some(room) = inner.conns.get(&id) else {
            return;
        };
        let Some(room_state) = inner.rooms.get(room) else {
            return;
        };
        let Some(sender) = room_state.members.get(&id) else {
            return;
        };
        room_state.broadcast(&encode(&build(&sender.user)));
    }

    // -- Coordination (Tier 3): claims + agent posts. Callable without joining
    // -- (the agent helper's one-shot connections), so they take the room code.

    /// Claim `path` for `user`. Same-user re-claims refresh the note/stamp;
    /// another user's live claim wins and is returned as the holder.
    pub fn claim(&self, room: &str, user: &str, puppy: &str, path: &str, note: &str) -> ServerMsg {
        let mut inner = self.inner.lock().unwrap();
        let Some(room_state) = inner.rooms.get_mut(room) else {
            return ServerMsg::ClaimResult {
                ok: false,
                holder: None,
            };
        };
        let items = room_state.claims_snapshot(); // prunes expired first
        if let Some(holder) = items.iter().find(|c| c.path == path && c.user != user) {
            return ServerMsg::ClaimResult {
                ok: false,
                holder: Some(holder.clone()),
            };
        }
        room_state.claims.insert(
            path.to_string(),
            Claim {
                user: user.to_string(),
                puppy: puppy.to_string(),
                note: note.to_string(),
                ts: now_ts(),
            },
        );
        let items = room_state.claims_snapshot();
        room_state.broadcast(&encode(&ServerMsg::Claims { items }));
        ServerMsg::ClaimResult {
            ok: true,
            holder: None,
        }
    }

    /// Release `user`'s claim on `path` (only the holder can release).
    pub fn release(&self, room: &str, user: &str, path: &str) -> ServerMsg {
        let mut inner = self.inner.lock().unwrap();
        let Some(room_state) = inner.rooms.get_mut(room) else {
            return ServerMsg::ReleaseResult { ok: false };
        };
        let held_by_user = room_state.claims.get(path).is_some_and(|c| c.user == user);
        if !held_by_user {
            return ServerMsg::ReleaseResult { ok: false };
        }
        room_state.claims.remove(path);
        let items = room_state.claims_snapshot();
        room_state.broadcast(&encode(&ServerMsg::Claims { items }));
        ServerMsg::ReleaseResult { ok: true }
    }

    /// The room's active claims.
    pub fn list_claims(&self, room: &str) -> ServerMsg {
        let mut inner = self.inner.lock().unwrap();
        let items = inner
            .rooms
            .get_mut(room)
            .map(Room::claims_snapshot)
            .unwrap_or_default();
        ServerMsg::Claims { items }
    }

    /// Post into a room from a one-shot agent connection: lands in the feed
    /// as a `Puppy` entry (the helper speaks AS the puppy; no member user).
    pub fn post(&self, room: &str, from: &str, text: &str) -> ServerMsg {
        let mut inner = self.inner.lock().unwrap();
        let Some(room_state) = inner.rooms.get_mut(room) else {
            return ServerMsg::PostResult { ok: false };
        };
        room_state.push_feed(FeedKind::Puppy, "", from, FeedSeed::text(text.to_string()));
        ServerMsg::PostResult { ok: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn drain(rx: &Receiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(line);
        }
        out
    }

    /// Unwrap a Joined snapshot (panics loudly on anything else).
    fn snapshot(msg: ServerMsg) -> (Vec<MemberInfo>, Vec<FeedEntry>, Vec<TaskInfo>) {
        match msg {
            ServerMsg::Joined {
                members,
                feed,
                tasks,
                ..
            } => (members, feed, tasks),
            other => panic!("expected joined, got {other:?}"),
        }
    }

    #[test]
    fn join_announces_and_returns_roster() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();

        let (_aid, joined_a) = hub.join("room", "alice", "Rex", JoinAvatars::default(), atx);
        let (members_a, feed_a, _) = snapshot(joined_a);
        assert_eq!(members_a.len(), 1);
        assert_eq!(
            (members_a[0].user.as_str(), members_a[0].puppy.as_str()),
            ("alice", "Rex")
        );
        assert!(members_a[0].host, "first member is the host");
        assert!(!members_a[0].color.is_empty(), "relay assigns a color");
        assert_eq!(feed_a.len(), 1, "own join is narrated in the snapshot");
        assert!(drain(&arx).is_empty(), "no self-announce on join");

        let (_bid, joined_b) = hub.join("room", "bob", "Biscuit", JoinAvatars::default(), btx);
        let (members_b, _, _) = snapshot(joined_b);
        let names: Vec<&str> = members_b.iter().map(|m| m.user.as_str()).collect();
        assert_eq!(names, vec!["alice", "bob"]);
        let host_flags: Vec<bool> = members_b.iter().map(|m| m.host).collect();
        assert_eq!(host_flags, vec![true, false], "alice keeps host");
        assert_ne!(
            members_b[0].color, members_b[1].color,
            "palette colors are distinct"
        );
        let a_saw = drain(&arx);
        assert_eq!(a_saw.len(), 2, "announce + system feed line");
        assert!(a_saw[0].contains("member_joined") && a_saw[0].contains("bob"));
        assert!(
            a_saw[0].contains("Biscuit"),
            "announce carries the puppy name"
        );
        assert!(a_saw[1].contains(r#""kind":"system""#) && a_saw[1].contains("joined the den"));
        assert!(drain(&brx).is_empty(), "joiner isn't told about themselves");
    }

    #[test]
    fn chat_reaches_everyone_including_sender() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();
        let (aid, _) = hub.join("room", "alice", "", JoinAvatars::default(), atx);
        let (_bid, _) = hub.join("room", "bob", "", JoinAvatars::default(), btx);
        drain(&arx);

        hub.chat(aid, "hello den");
        for rx in [&arx, &brx] {
            let got = drain(rx);
            assert_eq!(got.len(), 1);
            assert!(got[0].contains(r#""event":"feed""#));
            assert!(got[0].contains(r#""user":"alice""#));
            assert!(got[0].contains("hello den"));
        }
    }

    #[test]
    fn rooms_are_isolated() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();
        let (aid, _) = hub.join("room-one", "alice", "", JoinAvatars::default(), atx);
        let (_bid, _) = hub.join("room-two", "bob", "", JoinAvatars::default(), btx);

        hub.chat(aid, "secret");
        assert_eq!(drain(&arx).len(), 1);
        assert!(drain(&brx).is_empty(), "other rooms must not see it");
    }

    #[test]
    fn leave_announces_and_cleans_up() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, _brx) = channel();
        let (_aid, _) = hub.join("room", "alice", "", JoinAvatars::default(), atx);
        let (bid, _) = hub.join("room", "bob", "", JoinAvatars::default(), btx);
        drain(&arx);

        hub.leave(bid);
        let a_saw = drain(&arx);
        assert_eq!(a_saw.len(), 2, "member_left + system feed line");
        assert!(a_saw[0].contains("member_left") && a_saw[0].contains("bob"));
        assert!(a_saw[1].contains(r#""kind":"system""#) && a_saw[1].contains("left the den"));

        // Chat from a departed member goes nowhere and doesn't panic.
        hub.chat(bid, "ghost");
        assert!(drain(&arx).is_empty());
    }

    #[test]
    fn claims_conflict_release_and_broadcast() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (_aid, _) = hub.join("room", "alice", "Rex", JoinAvatars::default(), atx);

        // Claim succeeds, the room is told, and the list shows it.
        let r = hub.claim("room", "alice", "Rex", "src/auth.rs", "refactor");
        assert!(matches!(r, ServerMsg::ClaimResult { ok: true, .. }));
        let seen = drain(&arx);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains(r#""event":"claims""#) && seen[0].contains("src/auth.rs"));
        match hub.list_claims("room") {
            ServerMsg::Claims { items } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].user, "alice");
                assert_eq!(items[0].note, "refactor");
            }
            other => panic!("expected claims, got {other:?}"),
        }

        // Another user can't take it; the holder comes back.
        let r = hub.claim("room", "bob", "Biscuit", "src/auth.rs", "");
        match r {
            ServerMsg::ClaimResult { ok, holder } => {
                assert!(!ok);
                assert_eq!(holder.expect("holder").user, "alice");
            }
            other => panic!("expected claim_result, got {other:?}"),
        }
        // Same user re-claims fine (refresh).
        let r = hub.claim("room", "alice", "Rex", "src/auth.rs", "still on it");
        assert!(matches!(r, ServerMsg::ClaimResult { ok: true, .. }));

        // Only the holder can release.
        assert!(matches!(
            hub.release("room", "bob", "src/auth.rs"),
            ServerMsg::ReleaseResult { ok: false }
        ));
        assert!(matches!(
            hub.release("room", "alice", "src/auth.rs"),
            ServerMsg::ReleaseResult { ok: true }
        ));
        match hub.list_claims("room") {
            ServerMsg::Claims { items } => assert!(items.is_empty()),
            other => panic!("expected claims, got {other:?}"),
        }
    }

    #[test]
    fn coordination_needs_an_active_room() {
        let hub = Hub::new();
        assert!(matches!(
            hub.claim("ghost-room", "a", "", "x.rs", ""),
            ServerMsg::ClaimResult {
                ok: false,
                holder: None
            }
        ));
        assert!(matches!(
            hub.post("ghost-room", "a", "hi"),
            ServerMsg::PostResult { ok: false }
        ));
    }

    #[test]
    fn agent_post_reaches_members() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (_aid, _) = hub.join("room", "alice", "", JoinAvatars::default(), atx);
        let r = hub.post("room", "Rufus", "taking the parser");
        assert!(matches!(r, ServerMsg::PostResult { ok: true }));
        let seen = drain(&arx);
        assert_eq!(seen.len(), 1);
        assert!(
            seen[0].contains(r#""kind":"puppy""#)
                && seen[0].contains(r#""puppy":"Rufus""#)
                && seen[0].contains("taking the parser")
        );
    }

    #[test]
    fn presence_kanban_and_plans_round_trip() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (aid, _) = hub.join("room", "alice", "Rex", JoinAvatars::default(), atx);

        // Presence change broadcasts once; a no-op flap stays silent.
        hub.presence(aid, Presence::Idle);
        hub.presence(aid, Presence::Idle);
        let seen = drain(&arx);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains(r#""presence":"idle""#));

        // Kanban: create + move; ids are relay-assigned; full board broadcast.
        hub.task_create(aid, "Refunds", TaskColumn::Backlog, "", false);
        hub.task_move(aid, 1, TaskColumn::InProgress);
        hub.task_assign(aid, 1, "alice");
        hub.task_retitle(aid, 1, "Refunds v2");
        let seen = drain(&arx);
        assert_eq!(seen.len(), 4);
        assert!(seen[3].contains("Refunds v2") && seen[3].contains(r#""in_progress""#));
        hub.task_delete(aid, 1);
        hub.task_delete(aid, 99); // unknown id: silent no-op
        let seen = drain(&arx);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains(r#""items":[]"#));

        // Plans: share (broadcast + narration), update in place, unshare.
        hub.plan_share(aid, "Rex", "- [ ] a");
        hub.plan_share(aid, "Rex", "- [x] a");
        let seen = drain(&arx);
        assert_eq!(seen.len(), 4, "two shares = 2 plan lists + 2 feed lines");
        assert!(seen[2].contains("- [x] a"));
        hub.plan_unshare(aid, "Rex");
        let seen = drain(&arx);
        assert_eq!(seen.len(), 2);
        assert!(seen[1].contains("withdrew plans.md"));
    }

    #[test]
    fn late_joiner_gets_existing_members_rosters() {
        let hub = Hub::new();
        let (atx, _arx) = channel();
        let (aid, _) = hub.join("room", "alice", "Rex", JoinAvatars::default(), atx);
        // Alice reports her agents BEFORE bob joins.
        hub.roster(
            aid,
            vec![RoomAgentInfo {
                puppy: "Rex".into(),
                agent: "code-puppy".into(),
                model: "opus".into(),
                state: "idle".into(),
                verb: String::new(),
                file: String::new(),
                dir: "proj".into(),
                tps: 0.0,
                added: 0,
                removed: 0,
            }],
        );

        let (btx, brx) = channel();
        let (bid, _joined) = hub.join("room", "bob", "", JoinAvatars::default(), btx);
        hub.replay_rosters(bid); // server.rs calls this right after the snapshot
        // Bob's first messages include alice's replayed roster.
        let bob_saw = drain(&brx);
        assert!(
            bob_saw.iter().any(|l| l.contains(r#""event":"roster""#)
                && l.contains(r#""from":"alice""#)
                && l.contains("proj")),
            "late joiner must receive alice's cached roster: {bob_saw:?}"
        );
    }

    #[test]
    fn late_joiner_gets_the_den_snapshot() {
        let hub = Hub::new();
        let (atx, _arx) = channel();
        let (aid, _) = hub.join("room", "alice", "Rex", JoinAvatars::default(), atx);
        hub.chat(aid, "first!");
        hub.task_create(aid, "Webhooks", TaskColumn::InProgress, "alice", true);
        hub.plan_share(aid, "Rex", "- [ ] verify signatures");

        let (btx, _brx) = channel();
        let (_bid, joined) = hub.join("room", "bob", "", JoinAvatars::default(), btx);
        match joined {
            ServerMsg::Joined {
                feed, tasks, plans, ..
            } => {
                assert!(feed.iter().any(|e| e.text == "first!"));
                let seqs: Vec<u64> = feed.iter().map(|e| e.seq).collect();
                let mut sorted = seqs.clone();
                sorted.sort_unstable();
                assert_eq!(seqs, sorted, "snapshot preserves server ordering");
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].title, "Webhooks");
                assert!(tasks[0].plan);
                assert_eq!(plans.len(), 1);
                assert_eq!(plans[0].puppy, "Rex");
            }
            other => panic!("expected joined, got {other:?}"),
        }
    }
}
