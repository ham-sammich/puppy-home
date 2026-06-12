//! Room membership + broadcast. Pure logic over `mpsc` senders (one per
//! connection's outbound queue), so it unit-tests without any sockets.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{MemberInfo, ServerMsg};

/// All rooms + who's in them. One per relay process, shared across connections.
#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    /// room code -> connection id -> member
    rooms: HashMap<String, HashMap<u64, Member>>,
    /// connection id -> room code (for leave/broadcast lookups)
    conns: HashMap<u64, String>,
}

struct Member {
    user: String,
    puppy: String,
    tx: Sender<String>,
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
        let members = inner.rooms.entry(room.to_string()).or_default();
        for m in members.values() {
            let _ = m.tx.send(joined.clone());
        }
        members.insert(
            id,
            Member {
                user: user.to_string(),
                puppy: puppy.to_string(),
                tx,
            },
        );
        let mut roster: Vec<MemberInfo> = members
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
            .and_then(|members| members.remove(&id))
            .map(|m| m.user);
        if let Some(user) = user
            && let Some(members) = inner.rooms.get(&room)
        {
            let left = encode(&ServerMsg::MemberLeft { user });
            for m in members.values() {
                let _ = m.tx.send(left.clone());
            }
        }
        if inner.rooms.get(&room).is_some_and(HashMap::is_empty) {
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
        let Some(members) = inner.rooms.get(room) else {
            return;
        };
        let Some(sender) = members.get(&id) else {
            return;
        };
        let line = encode(&build(&sender.user));
        for m in members.values() {
            let _ = m.tx.send(line.clone());
        }
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
}
