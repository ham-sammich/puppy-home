//! Room membership + broadcast. Pure logic over `mpsc` senders (one per
//! connection's outbound queue), so it unit-tests without any sockets.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{ClaimInfo, MemberInfo, ServerMsg};

/// Claims auto-expire after this long (re-claiming refreshes the stamp), so a
/// crashed agent can't squat on a file forever.
const CLAIM_TTL_SECS: u64 = 3600;

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
}

impl Room {
    fn broadcast(&self, line: &str) {
        for m in self.members.values() {
            let _ = m.tx.send(line.to_string());
        }
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
    tx: Sender<String>,
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

    /// Add a member to a room (creating it on first join). Existing members are
    /// told; the new member's current roster (including themselves) is returned
    /// along with their connection id.
    pub fn join(
        &self,
        room: &str,
        user: &str,
        puppy: &str,
        tx: Sender<String>,
    ) -> (u64, Vec<MemberInfo>) {
        let mut inner = self.inner.lock().unwrap();
        inner.next_id += 1;
        let id = inner.next_id;

        let joined = encode(&ServerMsg::MemberJoined {
            user: user.into(),
            puppy: puppy.into(),
        });
        let room_state = inner.rooms.entry(room.to_string()).or_default();
        room_state.broadcast(&joined);
        room_state.members.insert(
            id,
            Member {
                user: user.to_string(),
                puppy: puppy.to_string(),
                tx,
            },
        );
        let mut roster: Vec<MemberInfo> = room_state
            .members
            .values()
            .map(|m| MemberInfo {
                user: m.user.clone(),
                puppy: m.puppy.clone(),
            })
            .collect();
        roster.sort_by(|a, b| a.user.cmp(&b.user));
        inner.conns.insert(id, room.to_string());
        (id, roster)
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
            && let Some(room_state) = inner.rooms.get(&room)
        {
            room_state.broadcast(&encode(&ServerMsg::MemberLeft { user }));
        }
        if inner.rooms.get(&room).is_some_and(|r| r.members.is_empty()) {
            inner.rooms.remove(&room);
        }
    }

    /// Re-broadcast a chat line to the whole room (sender included, so everyone
    /// shares the relay's ordering + timestamp).
    pub fn chat(&self, id: u64, text: &str) {
        self.broadcast_from(id, |user| ServerMsg::Chat {
            from: user.to_string(),
            text: text.to_string(),
            ts: now_ts(),
        });
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

    /// Post a chat line into a room (e.g. an agent announcing its plan).
    pub fn post(&self, room: &str, from: &str, text: &str) -> ServerMsg {
        let inner = self.inner.lock().unwrap();
        let Some(room_state) = inner.rooms.get(room) else {
            return ServerMsg::PostResult { ok: false };
        };
        room_state.broadcast(&encode(&ServerMsg::Chat {
            from: from.to_string(),
            text: text.to_string(),
            ts: now_ts(),
        }));
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

    #[test]
    fn join_announces_and_returns_roster() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();

        let (_aid, roster_a) = hub.join("room", "alice", "Rex", atx);
        assert_eq!(roster_a.len(), 1);
        assert_eq!(
            (roster_a[0].user.as_str(), roster_a[0].puppy.as_str()),
            ("alice", "Rex")
        );
        assert!(drain(&arx).is_empty(), "no self-announce on join");

        let (_bid, roster_b) = hub.join("room", "bob", "Biscuit", btx);
        let names: Vec<&str> = roster_b.iter().map(|m| m.user.as_str()).collect();
        assert_eq!(names, vec!["alice", "bob"]);
        let a_saw = drain(&arx);
        assert_eq!(a_saw.len(), 1);
        assert!(a_saw[0].contains("member_joined") && a_saw[0].contains("bob"));
        assert!(
            a_saw[0].contains("Biscuit"),
            "announce carries the puppy name"
        );
        assert!(drain(&brx).is_empty(), "joiner isn't told about themselves");
    }

    #[test]
    fn chat_reaches_everyone_including_sender() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();
        let (aid, _) = hub.join("room", "alice", "", atx);
        let (_bid, _) = hub.join("room", "bob", "", btx);
        drain(&arx);

        hub.chat(aid, "hello pack");
        for rx in [&arx, &brx] {
            let got = drain(rx);
            assert_eq!(got.len(), 1);
            assert!(got[0].contains(r#""from":"alice""#));
            assert!(got[0].contains("hello pack"));
        }
    }

    #[test]
    fn rooms_are_isolated() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, brx) = channel();
        let (aid, _) = hub.join("room-one", "alice", "", atx);
        let (_bid, _) = hub.join("room-two", "bob", "", btx);

        hub.chat(aid, "secret");
        assert_eq!(drain(&arx).len(), 1);
        assert!(drain(&brx).is_empty(), "other rooms must not see it");
    }

    #[test]
    fn leave_announces_and_cleans_up() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (btx, _brx) = channel();
        let (_aid, _) = hub.join("room", "alice", "", atx);
        let (bid, _) = hub.join("room", "bob", "", btx);
        drain(&arx);

        hub.leave(bid);
        let a_saw = drain(&arx);
        assert_eq!(a_saw.len(), 1);
        assert!(a_saw[0].contains("member_left") && a_saw[0].contains("bob"));

        // Chat from a departed member goes nowhere and doesn't panic.
        hub.chat(bid, "ghost");
        assert!(drain(&arx).is_empty());
    }

    #[test]
    fn claims_conflict_release_and_broadcast() {
        let hub = Hub::new();
        let (atx, arx) = channel();
        let (_aid, _) = hub.join("room", "alice", "Rex", atx);

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
        let (_aid, _) = hub.join("room", "alice", "", atx);
        let r = hub.post("room", "Rufus", "taking the parser");
        assert!(matches!(r, ServerMsg::PostResult { ok: true }));
        let seen = drain(&arx);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains(r#""from":"Rufus""#) && seen[0].contains("taking the parser"));
    }
}
