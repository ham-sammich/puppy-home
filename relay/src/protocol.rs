//! The Puppy Pack wire protocol: line-delimited JSON, one message per line.
//!
//! Client -> relay messages are tagged by `op`; relay -> client by `event`
//! (mirroring the puppy-home<->sidecar convention). The relay stamps `from` and
//! `ts` on everything it re-broadcasts, so clients never trust each other's
//! self-reported identity or clocks.

use serde::{Deserialize, Serialize};

/// Bump on incompatible wire changes; the relay rejects mismatched joins.
/// v2: members carry their puppy's name (Join/Joined/MemberJoined).
/// v3: file claims + roomless coordination ops (claim/release/list/post).
/// v4: the Den — member colors/presence/host, roster agent summaries, the
///     unified feed (replaces the `chat` event), kanban tasks, shared plans.
pub const PROTO_VERSION: u32 = 4;

/// A member's liveness as shown in the Den roster.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    #[default]
    Active,
    Idle,
}

/// Who/what produced a feed entry.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    /// A person typed it.
    Human,
    /// A puppy said it (optionally aimed at another puppy).
    Puppy,
    /// The relay narrating room life (joins, leaves, plan shares).
    System,
}

/// One entry in the Den's coordination feed. The relay assigns `seq` (room-
/// monotonic) and `ts`, so every client shares one ordering.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FeedEntry {
    pub seq: u64,
    pub kind: FeedKind,
    /// Sender's user name (empty for system entries and roomless posts).
    #[serde(default)]
    pub user: String,
    /// Speaking puppy (puppy entries only).
    #[serde(default)]
    pub puppy: String,
    /// Addressed puppy, empty when broadcast to the room.
    #[serde(default)]
    pub to_puppy: String,
    /// Flags a puppy message as a review remark (feed badge).
    #[serde(default)]
    pub review: bool,
    pub text: String,
    pub ts: u64,
}

/// A compact summary of one running agent, broadcast for the Den roster.
/// Kept flat + small: a member re-sends their whole list every few seconds.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RoomAgentInfo {
    #[serde(default)]
    pub puppy: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub model: String,
    /// Lifecycle label ("running", "tool", "paused", ...).
    #[serde(default)]
    pub state: String,
    /// Current tool verb ("edit", "shell", ...), empty when idle.
    #[serde(default)]
    pub verb: String,
    /// File the agent is touching right now, empty when unknown.
    #[serde(default)]
    pub file: String,
    /// Workspace folder name (not the full path — keep it shareable).
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub tps: f32,
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub removed: u64,
}

/// A kanban column. Fixed set — the board IS the workflow.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskColumn {
    #[default]
    Backlog,
    InProgress,
    Review,
    Done,
}

/// One kanban card. The relay owns ids and is the source of truth.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskInfo {
    pub id: u64,
    pub title: String,
    pub column: TaskColumn,
    /// Owning member's user name, empty = unassigned.
    #[serde(default)]
    pub owner: String,
    /// Card came from / belongs to a shared plan (the `📄 plan` tag).
    #[serde(default)]
    pub plan: bool,
}

/// A puppy's shared plans.md. Checklist parsing is the UI's job — the wire
/// carries the raw markdown.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlanInfo {
    pub user: String,
    pub puppy: String,
    pub markdown: String,
    pub ts: u64,
}

/// An active file claim in a room.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClaimInfo {
    /// Workspace-relative (or otherwise agreed) file path being worked on.
    pub path: String,
    pub user: String,
    #[serde(default)]
    pub puppy: String,
    /// Why it's claimed ("refactoring auth"), may be empty.
    #[serde(default)]
    pub note: String,
    pub ts: u64,
}

/// A den member as the relay knows them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct MemberInfo {
    pub user: String,
    /// The member's puppy name (Code Puppy's `puppy_name`), may be empty.
    #[serde(default)]
    pub puppy: String,
    /// Owner color (`#rrggbb`), relay-assigned from a palette by join order so
    /// colors are unique per room and clients never bikeshed them.
    #[serde(default)]
    pub color: String,
    /// Earliest-joined current member; passes on when the host leaves.
    #[serde(default)]
    pub host: bool,
    #[serde(default)]
    pub presence: Presence,
}

/// What a pack member sends to the relay.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Must be the first message on a connection. The room code is the shared
    /// secret: knowing it is membership.
    Join {
        room: String,
        user: String,
        /// The joiner's puppy name (shown to the pack; may be empty).
        #[serde(default)]
        puppy: String,
        #[serde(default)]
        proto: u32,
    },
    /// A chat line to everyone in the room (including the sender, for ordering).
    /// Lands in the feed as a `Human` entry.
    Chat { text: String },
    /// A live-activity ping: what this member's puppy is doing right now
    /// (e.g. kind="tool" detail="edit_file src/main.rs", kind="turn" "idle").
    Activity { kind: String, detail: String },
    /// This member went active/idle (the roster presence dot).
    Presence { presence: Presence },
    /// The member's compact agent summaries for the Den roster. Re-sent whole
    /// every few seconds (client rate-limits); the relay caps the list size.
    Roster { agents: Vec<RoomAgentInfo> },
    /// A puppy speaking into the feed (optionally at another puppy).
    PuppyMsg {
        puppy: String,
        #[serde(default)]
        to_puppy: String,
        #[serde(default)]
        review: bool,
        text: String,
    },
    /// Create a kanban card; the relay assigns the id.
    TaskCreate {
        title: String,
        #[serde(default)]
        column: TaskColumn,
        #[serde(default)]
        owner: String,
        #[serde(default)]
        plan: bool,
    },
    /// Move a card to a column.
    TaskMove { id: u64, column: TaskColumn },
    /// Re-assign a card (empty owner = unassign).
    TaskAssign { id: u64, owner: String },
    /// Rename a card.
    TaskRetitle { id: u64, title: String },
    /// Delete a card.
    TaskDelete { id: u64 },
    /// Share (or update) this member's puppy plans.md with the den.
    PlanShare { puppy: String, markdown: String },
    /// Withdraw a previously shared plan.
    PlanUnshare { puppy: String },
    /// Graceful goodbye (disconnecting works too).
    Leave,
    // -- Coordination ops (Tier 3). Also valid as a connection's FIRST message
    // -- ("roomless": op -> one reply -> close), which is how the agent-side
    // -- helper CLI uses them. The room code is the capability.
    /// Claim a file for this user; fails if another user holds it.
    Claim {
        room: String,
        user: String,
        #[serde(default)]
        puppy: String,
        path: String,
        #[serde(default)]
        note: String,
    },
    /// Release this user's claim on a file.
    Release {
        room: String,
        user: String,
        path: String,
    },
    /// Ask for the room's active claims.
    ListClaims { room: String },
    /// Post a chat line into the room (e.g. the agent announcing its plan).
    Post {
        room: String,
        from: String,
        text: String,
    },
}

/// What the relay sends to pack members.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to a successful join: who's in the room (including you) plus the
    /// den snapshot — feed tail, kanban, shared plans — so late joiners are
    /// current immediately. (Rosters are NOT snapshotted; they re-arrive on
    /// each member's next broadcast within a few seconds.)
    Joined {
        room: String,
        members: Vec<MemberInfo>,
        #[serde(default)]
        feed: Vec<FeedEntry>,
        #[serde(default)]
        tasks: Vec<TaskInfo>,
        #[serde(default)]
        plans: Vec<PlanInfo>,
    },
    MemberJoined {
        user: String,
        #[serde(default)]
        puppy: String,
        #[serde(default)]
        color: String,
    },
    MemberLeft {
        user: String,
    },
    /// One new feed entry (human chat, puppy message, or system event). The
    /// den's only chat channel — v4 retired the old `chat` event in its favor.
    Feed {
        entry: FeedEntry,
    },
    /// A member's presence changed.
    Presence {
        user: String,
        presence: Presence,
    },
    /// A member's current agent summaries (roster cards).
    Roster {
        from: String,
        agents: Vec<RoomAgentInfo>,
        ts: u64,
    },
    /// The full kanban after any change (small board — the snapshot IS the
    /// delta; idempotent and immune to reordering).
    Tasks {
        items: Vec<TaskInfo>,
    },
    /// Every shared plan after any share/update/unshare.
    Plans {
        items: Vec<PlanInfo>,
    },
    Activity {
        from: String,
        kind: String,
        detail: String,
        ts: u64,
    },
    Error {
        message: String,
    },
    /// Reply to a `Claim`: on failure `holder` says who has it.
    ClaimResult {
        ok: bool,
        #[serde(default)]
        holder: Option<ClaimInfo>,
    },
    /// Reply to a `Release`.
    ReleaseResult {
        ok: bool,
    },
    /// The room's active claims: reply to `ListClaims` AND broadcast to members
    /// whenever they change.
    Claims {
        items: Vec<ClaimInfo>,
    },
    /// Reply to a `Post`.
    PostResult {
        ok: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt_client(msg: &ClientMsg) -> ClientMsg {
        serde_json::from_str(&serde_json::to_string(msg).unwrap()).unwrap()
    }

    fn rt_server(msg: &ServerMsg) -> ServerMsg {
        serde_json::from_str(&serde_json::to_string(msg).unwrap()).unwrap()
    }

    #[test]
    fn client_messages_round_trip() {
        for msg in [
            ClientMsg::Join {
                room: "swift-otter-42".into(),
                user: "jacob".into(),
                puppy: "Rufus".into(),
                proto: PROTO_VERSION,
            },
            ClientMsg::Chat {
                text: "hello den".into(),
            },
            ClientMsg::Activity {
                kind: "tool".into(),
                detail: "edit_file src/main.rs".into(),
            },
            ClientMsg::Presence {
                presence: Presence::Idle,
            },
            ClientMsg::Roster {
                agents: vec![RoomAgentInfo {
                    puppy: "Rex".into(),
                    agent: "code-puppy".into(),
                    model: "claude-opus".into(),
                    state: "tool".into(),
                    verb: "edit".into(),
                    file: "src/main.rs".into(),
                    dir: "checkout-svc".into(),
                    tps: 58.5,
                    added: 41,
                    removed: 7,
                }],
            },
            ClientMsg::PuppyMsg {
                puppy: "Rex".into(),
                to_puppy: "Biscuit".into(),
                review: true,
                text: "replay risk on event.id".into(),
            },
            ClientMsg::TaskCreate {
                title: "Refund endpoint".into(),
                column: TaskColumn::Backlog,
                owner: String::new(),
                plan: false,
            },
            ClientMsg::TaskMove {
                id: 3,
                column: TaskColumn::Review,
            },
            ClientMsg::TaskAssign {
                id: 3,
                owner: "jordan".into(),
            },
            ClientMsg::TaskRetitle {
                id: 3,
                title: "Refunds v2".into(),
            },
            ClientMsg::TaskDelete { id: 3 },
            ClientMsg::PlanShare {
                puppy: "Rex".into(),
                markdown: "- [x] wire form\n- [ ] spinner".into(),
            },
            ClientMsg::PlanUnshare {
                puppy: "Rex".into(),
            },
            ClientMsg::Leave,
            ClientMsg::Claim {
                room: "r".into(),
                user: "jacob".into(),
                puppy: "Rufus".into(),
                path: "src/auth.rs".into(),
                note: "refactoring".into(),
            },
            ClientMsg::Release {
                room: "r".into(),
                user: "jacob".into(),
                path: "src/auth.rs".into(),
            },
            ClientMsg::ListClaims { room: "r".into() },
            ClientMsg::Post {
                room: "r".into(),
                from: "Rufus".into(),
                text: "starting on the UI".into(),
            },
        ] {
            assert_eq!(rt_client(&msg), msg);
        }
    }

    #[test]
    fn server_messages_round_trip() {
        for msg in [
            ServerMsg::Joined {
                room: "r".into(),
                members: vec![
                    MemberInfo {
                        user: "a".into(),
                        puppy: "Rex".into(),
                        color: "#e7ab4d".into(),
                        host: true,
                        presence: Presence::Active,
                    },
                    MemberInfo {
                        user: "b".into(),
                        puppy: String::new(),
                        color: "#56c7c2".into(),
                        host: false,
                        presence: Presence::Idle,
                    },
                ],
                feed: vec![FeedEntry {
                    seq: 1,
                    kind: FeedKind::System,
                    user: String::new(),
                    puppy: String::new(),
                    to_puppy: String::new(),
                    review: false,
                    text: "a joined the den".into(),
                    ts: 5,
                }],
                tasks: vec![TaskInfo {
                    id: 1,
                    title: "Refund endpoint".into(),
                    column: TaskColumn::Backlog,
                    owner: String::new(),
                    plan: false,
                }],
                plans: vec![PlanInfo {
                    user: "a".into(),
                    puppy: "Rex".into(),
                    markdown: "- [ ] x".into(),
                    ts: 6,
                }],
            },
            ServerMsg::MemberJoined {
                user: "b".into(),
                puppy: "Biscuit".into(),
                color: "#b79cff".into(),
            },
            ServerMsg::MemberLeft { user: "b".into() },
            ServerMsg::Feed {
                entry: FeedEntry {
                    seq: 9,
                    kind: FeedKind::Puppy,
                    user: "a".into(),
                    puppy: "Rex".into(),
                    to_puppy: "Biscuit".into(),
                    review: true,
                    text: "dedupe on event.id".into(),
                    ts: 7,
                },
            },
            ServerMsg::Presence {
                user: "a".into(),
                presence: Presence::Idle,
            },
            ServerMsg::Roster {
                from: "a".into(),
                agents: vec![RoomAgentInfo::default()],
                ts: 8,
            },
            ServerMsg::Tasks {
                items: vec![TaskInfo {
                    id: 2,
                    title: "t".into(),
                    column: TaskColumn::Done,
                    owner: "a".into(),
                    plan: true,
                }],
            },
            ServerMsg::Plans { items: vec![] },
            ServerMsg::Activity {
                from: "a".into(),
                kind: "turn".into(),
                detail: "running".into(),
                ts: 8,
            },
            ServerMsg::Error {
                message: "nope".into(),
            },
            ServerMsg::ClaimResult {
                ok: false,
                holder: Some(ClaimInfo {
                    path: "src/auth.rs".into(),
                    user: "mike".into(),
                    puppy: "Biscuit".into(),
                    note: "auth refactor".into(),
                    ts: 9,
                }),
            },
            ServerMsg::ReleaseResult { ok: true },
            ServerMsg::Claims {
                items: vec![ClaimInfo {
                    path: "a.rs".into(),
                    user: "u".into(),
                    puppy: String::new(),
                    note: String::new(),
                    ts: 1,
                }],
            },
            ServerMsg::PostResult { ok: true },
        ] {
            assert_eq!(rt_server(&msg), msg);
        }
    }

    /// Pin the exact wire shapes -- the Rust client parses these by hand-shaken
    /// contract, so a silent rename would break the seam.
    #[test]
    fn wire_shapes_are_pinned() {
        let join = ClientMsg::Join {
            room: "r1".into(),
            user: "alice".into(),
            puppy: "Rex".into(),
            proto: 2,
        };
        assert_eq!(
            serde_json::to_string(&join).unwrap(),
            r#"{"op":"join","room":"r1","user":"alice","puppy":"Rex","proto":2}"#
        );
        let feed = ServerMsg::Feed {
            entry: FeedEntry {
                seq: 3,
                kind: FeedKind::Human,
                user: "alice".into(),
                puppy: String::new(),
                to_puppy: String::new(),
                review: false,
                text: "hi".into(),
                ts: 42,
            },
        };
        assert_eq!(
            serde_json::to_string(&feed).unwrap(),
            r#"{"event":"feed","entry":{"seq":3,"kind":"human","user":"alice","puppy":"","to_puppy":"","review":false,"text":"hi","ts":42}}"#
        );
        // Kanban columns ride snake_case.
        assert_eq!(
            serde_json::to_string(&TaskColumn::InProgress).unwrap(),
            r#""in_progress""#
        );
        // `proto` and `puppy` are optional on the wire.
        let parsed: ClientMsg =
            serde_json::from_str(r#"{"op":"join","room":"r","user":"u"}"#).unwrap();
        assert_eq!(
            parsed,
            ClientMsg::Join {
                room: "r".into(),
                user: "u".into(),
                puppy: String::new(),
                proto: 0
            }
        );
    }
}
