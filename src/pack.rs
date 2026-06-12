//! Puppy Pack client: a TCP line-JSON connection to a `puppy-relay`.
//!
//! Reuses the relay crate's wire types (`ClientMsg`/`ServerMsg`), so there is
//! exactly one protocol definition for both ends. A reader thread parses
//! incoming lines into [`PackEvent`]s on a channel and wakes the UI -- the same
//! events-in-over-a-channel pattern every other backend in the app uses.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use puppy_relay::protocol::{
    ClaimInfo, ClientMsg, FeedEntry, FeedKind, MemberInfo, PROTO_VERSION, PlanInfo, Presence,
    RoomAgentInfo, ServerMsg, TaskColumn, TaskInfo,
};

use crate::waker::UiWaker;

/// The user-facing name for the collaborative room (the redesign renamed
/// "Pack" → "Den"). The boundary is deliberate: strings a USER sees say Den;
/// internal identifiers (the `puppy-relay` crate, `PackClient`, `Tab::Pack`,
/// module/file names) keep "pack" — renaming them is churn without behavior.
pub const DEN_LABEL: &str = "Den";

/// How many feed entries the client keeps (a live room, not an archive).
const DEN_FEED_CAP: usize = 500;

/// Client-side den state, folded from relay events — the one structure the
/// Den UI reads: members, per-member roster cards, the bounded feed ring,
/// the kanban board, and shared plans. The relay is the source of truth;
/// this is a faithful mirror.
#[derive(Default)]
pub struct DenState {
    /// Current members (relay-sorted by user; colors/host/presence included).
    pub members: Vec<MemberInfo>,
    /// user -> (their latest roster broadcast, relay ts).
    pub roster: HashMap<String, (Vec<RoomAgentInfo>, u64)>,
    /// Feed ring, relay-ordered by `seq`, oldest at the front.
    pub feed: VecDeque<FeedEntry>,
    pub tasks: Vec<TaskInfo>,
    pub plans: Vec<PlanInfo>,
    /// user -> (kind, detail) of their latest activity ping (the legacy
    /// status broadcast teammates see; feeds the Tier-2 breadcrumb).
    pub activity: HashMap<String, (String, String)>,
    /// The room's active file claims (broadcast by the relay).
    pub claims: Vec<ClaimInfo>,
}

impl DenState {
    /// Fold one relay event into the mirror. Unknown-to-the-den events
    /// (activity pings, claim replies) are ignored.
    pub fn apply(&mut self, msg: &ServerMsg) {
        match msg {
            ServerMsg::Joined {
                members,
                feed,
                tasks,
                plans,
                ..
            } => {
                self.members = members.clone();
                self.feed = feed.iter().cloned().collect();
                self.tasks = tasks.clone();
                self.plans = plans.clone();
                self.roster.clear(); // rosters re-arrive on each member's next tick
            }
            ServerMsg::MemberJoined { user, puppy, color } => {
                if !self.members.iter().any(|m| &m.user == user) {
                    self.members.push(MemberInfo {
                        user: user.clone(),
                        puppy: puppy.clone(),
                        color: color.clone(),
                        host: false,
                        presence: Presence::Active,
                    });
                    self.members.sort_by(|a, b| a.user.cmp(&b.user));
                }
            }
            ServerMsg::MemberLeft { user } => {
                self.members.retain(|m| &m.user != user);
                self.roster.remove(user);
            }
            ServerMsg::Feed { entry } => {
                self.feed.push_back(entry.clone());
                while self.feed.len() > DEN_FEED_CAP {
                    self.feed.pop_front();
                }
            }
            ServerMsg::Presence { user, presence } => {
                if let Some(m) = self.members.iter_mut().find(|m| &m.user == user) {
                    m.presence = *presence;
                }
            }
            ServerMsg::Roster { from, agents, ts } => {
                self.roster.insert(from.clone(), (agents.clone(), *ts));
            }
            ServerMsg::Tasks { items } => self.tasks = items.clone(),
            ServerMsg::Plans { items } => self.plans = items.clone(),
            ServerMsg::Activity {
                from, kind, detail, ..
            } => {
                self.activity
                    .insert(from.clone(), (kind.clone(), detail.clone()));
            }
            ServerMsg::Claims { items } => self.claims = items.clone(),
            _ => {}
        }
    }

    /// The `.puppy/pack.json` breadcrumb body each sidecar reads to inject
    /// "[pack context] ..." into prompts (Tier 2). Shape matches the egui
    /// shell's `PackView::breadcrumb` exactly (members w/ latest activity,
    /// active claims, the last 10 chat lines). The caller stamps `updated`
    /// at write time so this stays change-comparable.
    pub fn breadcrumb_body(
        &self,
        room: &str,
        relay: &str,
        user: &str,
        puppy: &str,
    ) -> serde_json::Value {
        let members: Vec<serde_json::Value> = self
            .members
            .iter()
            .map(|m| {
                let activity = self
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
                serde_json::json!({ "user": m.user, "puppy": m.puppy, "activity": activity })
            })
            .collect();
        // Chat lines, decorated the way the egui feed shows them.
        let chat: Vec<serde_json::Value> = self
            .feed
            .iter()
            .filter_map(|entry| match entry.kind {
                FeedKind::Human => {
                    Some(serde_json::json!({ "from": entry.user, "text": entry.text }))
                }
                FeedKind::Puppy => {
                    let from = if entry.to_puppy.is_empty() {
                        format!("\u{1f436} {}", entry.puppy)
                    } else {
                        format!("\u{1f436} {} \u{2192} {}", entry.puppy, entry.to_puppy)
                    };
                    Some(serde_json::json!({ "from": from, "text": entry.text }))
                }
                FeedKind::System => None,
            })
            .collect();
        let recent: Vec<serde_json::Value> = chat.iter().rev().take(10).rev().cloned().collect();
        let claims: Vec<serde_json::Value> = self
            .claims
            .iter()
            .map(|c| {
                serde_json::json!({
                    "path": c.path, "user": c.user, "puppy": c.puppy, "note": c.note,
                })
            })
            .collect();
        serde_json::json!({
            "room": room,
            "relay": relay,
            "user": user.trim(),
            "puppy": puppy.trim(),
            "members": members,
            "claims": claims,
            "chat": recent,
        })
    }
}

/// The agent-side coordination CLI (claim/release/claims/post/status),
/// shipped into each workspace's `.puppy/` so agents can run it with plain
/// python (one copy of the bytes; the egui shell's `app/pack_sync.rs`
/// include converges here at sync time).
pub const PACK_HELPER: &str = include_str!("../sidecar/pack_helper.py");

/// Drop the Tier-2 breadcrumb (`pack.json` + `pack_helper.py`) into each
/// LOCAL workspace root's `.puppy/`. The body gets `updated` stamped and a
/// per-root `helper` path (so the breadcrumb can point at ITS helper).
pub fn write_pack_breadcrumb(roots: &[std::path::PathBuf], body: &serde_json::Value) {
    let mut obj = body.clone();
    if let Some(map) = obj.as_object_mut() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        map.insert("updated".into(), serde_json::json!(now));
    }
    for root in roots {
        let dir = root.join(".puppy");
        let _ = std::fs::create_dir_all(&dir);
        let helper = dir.join("pack_helper.py");
        let _ = std::fs::write(&helper, PACK_HELPER);
        let mut per_root = obj.clone();
        if let Some(map) = per_root.as_object_mut() {
            map.insert("helper".into(), serde_json::json!(helper.to_string_lossy()));
        }
        let text = serde_json::to_string_pretty(&per_root).unwrap_or_default();
        let _ = std::fs::write(dir.join("pack.json"), &text);
    }
}

/// Remove the breadcrumb files from each root (called on leave).
pub fn remove_pack_breadcrumb(roots: &[std::path::PathBuf]) {
    for root in roots {
        let dir = root.join(".puppy");
        let _ = std::fs::remove_file(dir.join("pack.json"));
        let _ = std::fs::remove_file(dir.join("pack_helper.py"));
    }
}

/// What the reader thread delivers to the UI.
pub enum PackEvent {
    Msg(ServerMsg),
    /// The relay went away (EOF / IO error); the connection is dead.
    Disconnected,
}

/// A live connection to a relay room. Dropping it closes the socket.
pub struct PackClient {
    write: Mutex<TcpStream>,
}

impl PackClient {
    /// Connect, send the join, and start the reader thread. `addr` may omit the
    /// port (defaults to the relay's 9220). `puppy` is this member's puppy name,
    /// shown to the pack (may be empty). `waker` nudges the UI per event.
    pub fn connect(
        addr: &str,
        room: &str,
        user: &str,
        puppy: &str,
        waker: Arc<dyn UiWaker>,
    ) -> Result<(PackClient, Receiver<PackEvent>), String> {
        let addr = if addr.contains(':') {
            addr.to_string()
        } else {
            format!("{addr}:9220")
        };
        use std::net::ToSocketAddrs;
        let sock_addr = addr
            .to_socket_addrs()
            .map_err(|e| format!("resolve {addr}: {e}"))?
            .next()
            .ok_or_else(|| format!("no address for {addr}"))?;
        let stream = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(8))
            .map_err(|e| format!("connect {addr}: {e}"))?;
        let _ = stream.set_nodelay(true);

        let read_half = stream
            .try_clone()
            .map_err(|e| format!("clone stream: {e}"))?;
        let client = PackClient {
            write: Mutex::new(stream),
        };
        client.send(&ClientMsg::Join {
            room: room.to_string(),
            user: user.to_string(),
            puppy: puppy.to_string(),
            proto: PROTO_VERSION,
        });

        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("pack-read".into())
            .spawn(move || {
                let reader = BufReader::new(read_half);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(msg) = serde_json::from_str::<ServerMsg>(&line)
                        && tx.send(PackEvent::Msg(msg)).is_ok()
                    {
                        waker.wake();
                        continue;
                    }
                    // Unparseable line or UI gone: either way, stop cleanly.
                    if tx.send(PackEvent::Disconnected).is_err() {
                        return;
                    }
                }
                let _ = tx.send(PackEvent::Disconnected);
                waker.wake();
            })
            .map_err(|e| format!("spawn reader: {e}"))?;

        Ok((client, rx))
    }

    fn send(&self, msg: &ClientMsg) {
        if let Ok(mut stream) = self.write.lock()
            && let Ok(line) = serde_json::to_string(msg)
        {
            let _ = writeln!(stream, "{line}");
            let _ = stream.flush();
        }
    }

    /// Send a chat line to the room.
    pub fn chat(&self, text: &str) {
        self.send(&ClientMsg::Chat {
            text: text.to_string(),
        });
    }

    /// Broadcast this member's compact agent summaries for the den roster.
    pub fn send_roster(&self, agents: Vec<RoomAgentInfo>) {
        self.send(&ClientMsg::Roster { agents });
    }

    /// Announce going active/idle (the roster presence dot).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn send_presence(&self, presence: Presence) {
        self.send(&ClientMsg::Presence { presence });
    }

    /// A puppy speaking into the feed (`to_puppy` empty = the whole room).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn puppy_msg(&self, puppy: &str, to_puppy: &str, review: bool, text: &str) {
        self.send(&ClientMsg::PuppyMsg {
            puppy: puppy.to_string(),
            to_puppy: to_puppy.to_string(),
            review,
            text: text.to_string(),
        });
    }

    /// Create a kanban card (the relay assigns the id).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn task_create(&self, title: &str, column: TaskColumn, owner: &str, plan: bool) {
        self.send(&ClientMsg::TaskCreate {
            title: title.to_string(),
            column,
            owner: owner.to_string(),
            plan,
        });
    }

    /// Move a kanban card to a column.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn task_move(&self, id: u64, column: TaskColumn) {
        self.send(&ClientMsg::TaskMove { id, column });
    }

    /// Re-assign a kanban card (empty owner = unassign).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn task_assign(&self, id: u64, owner: &str) {
        self.send(&ClientMsg::TaskAssign {
            id,
            owner: owner.to_string(),
        });
    }

    /// Rename a kanban card.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn task_retitle(&self, id: u64, title: &str) {
        self.send(&ClientMsg::TaskRetitle {
            id,
            title: title.to_string(),
        });
    }

    /// Delete a kanban card.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn task_delete(&self, id: u64) {
        self.send(&ClientMsg::TaskDelete { id });
    }

    /// Share (or update) a puppy's plans.md with the den.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn plan_share(&self, puppy: &str, markdown: &str) {
        self.send(&ClientMsg::PlanShare {
            puppy: puppy.to_string(),
            markdown: markdown.to_string(),
        });
    }

    /// Withdraw a previously shared plan.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn plan_unshare(&self, puppy: &str) {
        self.send(&ClientMsg::PlanUnshare {
            puppy: puppy.to_string(),
        });
    }

    /// Broadcast what this member's puppy is doing right now.
    pub fn activity(&self, kind: &str, detail: &str) {
        self.send(&ClientMsg::Activity {
            kind: kind.to_string(),
            detail: detail.to_string(),
        });
    }

    /// Graceful goodbye (the relay also handles plain disconnects).
    pub fn leave(&self) {
        self.send(&ClientMsg::Leave);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// The whole seam: this client against a real in-process relay over TCP.
    #[test]
    fn client_joins_and_hears_its_own_chat() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let _ = puppy_relay::server::run(listener);
        });

        let (client, rx) = PackClient::connect(
            &format!("127.0.0.1:{port}"),
            "test-room",
            "tester",
            "Rex",
            Arc::new(crate::waker::NoopWaker),
        )
        .expect("connect+join");

        let mut den = DenState::default();
        match rx
            .recv_timeout(Duration::from_secs(5))
            .expect("joined event")
        {
            PackEvent::Msg(msg @ ServerMsg::Joined { .. }) => {
                den.apply(&msg);
                let ServerMsg::Joined { room, .. } = msg else {
                    unreachable!()
                };
                assert_eq!(room, "test-room");
                assert_eq!(den.members.len(), 1);
                assert_eq!(den.members[0].user, "tester");
                assert_eq!(den.members[0].puppy, "Rex");
                assert!(den.members[0].host, "sole member is the host");
                assert!(!den.members[0].color.is_empty());
                assert_eq!(den.feed.len(), 1, "own join narrated in the snapshot");
            }
            _ => panic!("expected joined first"),
        }

        client.chat("woof");
        match rx.recv_timeout(Duration::from_secs(5)).expect("feed event") {
            PackEvent::Msg(msg @ ServerMsg::Feed { .. }) => {
                den.apply(&msg);
                let last = den.feed.back().expect("entry in the ring");
                assert_eq!(last.user, "tester");
                assert_eq!(last.text, "woof");
                assert!(matches!(last.kind, puppy_relay::protocol::FeedKind::Human));
            }
            _ => panic!("expected the chat back as a feed entry"),
        }
    }

    #[test]
    fn breadcrumb_body_matches_the_egui_shape() {
        let mut st = DenState::default();
        st.apply(&ServerMsg::MemberJoined {
            user: "alice".into(),
            puppy: "Rex".into(),
            color: "#abc".into(),
        });
        st.apply(&ServerMsg::Activity {
            from: "alice".into(),
            kind: "status".into(),
            detail: "proj: running (edit_file)".into(),
            ts: 1,
        });
        st.apply(&ServerMsg::Claims {
            items: vec![ClaimInfo {
                path: "src/lib.rs".into(),
                user: "alice".into(),
                puppy: "Rex".into(),
                note: "refactor".into(),
                ts: 0,
            }],
        });
        let entry = |seq, kind, user: &str, puppy: &str, to: &str, text: &str| FeedEntry {
            seq,
            kind,
            user: user.into(),
            puppy: puppy.into(),
            to_puppy: to.into(),
            review: false,
            text: text.into(),
            ts: 0,
        };
        st.apply(&ServerMsg::Feed {
            entry: entry(1, FeedKind::Human, "alice", "", "", "hi"),
        });
        st.apply(&ServerMsg::Feed {
            entry: entry(2, FeedKind::Puppy, "", "Rex", "Biscuit", "woof"),
        });

        let body = st.breadcrumb_body("den-1", "127.0.0.1:9220", " bob ", "Biscuit");
        assert_eq!(body["room"], "den-1");
        assert_eq!(body["user"], "bob"); // trimmed, like egui
        // "status" activity renders bare (no "status: " prefix).
        assert_eq!(body["members"][0]["activity"], "proj: running (edit_file)");
        assert_eq!(body["claims"][0]["path"], "src/lib.rs");
        assert_eq!(body["chat"][0]["from"], "alice");
        // Puppy chat lines carry the egui feed decoration.
        assert_eq!(body["chat"][1]["from"], "\u{1f436} Rex \u{2192} Biscuit");
        assert_eq!(body["chat"][1]["text"], "woof");
    }
}
