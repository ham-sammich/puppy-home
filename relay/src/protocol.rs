//! The Puppy Pack wire protocol: line-delimited JSON, one message per line.
//!
//! Client -> relay messages are tagged by `op`; relay -> client by `event`
//! (mirroring the puppy-home<->sidecar convention). The relay stamps `from` and
//! `ts` on everything it re-broadcasts, so clients never trust each other's
//! self-reported identity or clocks.

use serde::{Deserialize, Serialize};

/// Bump on incompatible wire changes; the relay rejects mismatched joins.
/// v2: members carry their puppy's name (Join/Joined/MemberJoined).
pub const PROTO_VERSION: u32 = 2;

/// A pack member as the relay knows them.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MemberInfo {
    pub user: String,
    /// The member's puppy name (Code Puppy's `puppy_name`), may be empty.
    #[serde(default)]
    pub puppy: String,
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
    Chat { text: String },
    /// A live-activity ping: what this member's puppy is doing right now
    /// (e.g. kind="tool" detail="edit_file src/main.rs", kind="turn" "idle").
    Activity { kind: String, detail: String },
    /// Graceful goodbye (disconnecting works too).
    Leave,
}

/// What the relay sends to pack members.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to a successful join: who's in the room (including you).
    Joined {
        room: String,
        members: Vec<MemberInfo>,
    },
    MemberJoined {
        user: String,
        #[serde(default)]
        puppy: String,
    },
    MemberLeft {
        user: String,
    },
    Chat {
        from: String,
        text: String,
        ts: u64,
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
                text: "hello pack".into(),
            },
            ClientMsg::Activity {
                kind: "tool".into(),
                detail: "edit_file src/main.rs".into(),
            },
            ClientMsg::Leave,
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
                    },
                    MemberInfo {
                        user: "b".into(),
                        puppy: String::new(),
                    },
                ],
            },
            ServerMsg::MemberJoined {
                user: "b".into(),
                puppy: "Biscuit".into(),
            },
            ServerMsg::MemberLeft { user: "b".into() },
            ServerMsg::Chat {
                from: "a".into(),
                text: "hi".into(),
                ts: 7,
            },
            ServerMsg::Activity {
                from: "a".into(),
                kind: "turn".into(),
                detail: "running".into(),
                ts: 8,
            },
            ServerMsg::Error {
                message: "nope".into(),
            },
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
        let chat = ServerMsg::Chat {
            from: "alice".into(),
            text: "hi".into(),
            ts: 42,
        };
        assert_eq!(
            serde_json::to_string(&chat).unwrap(),
            r#"{"event":"chat","from":"alice","text":"hi","ts":42}"#
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
