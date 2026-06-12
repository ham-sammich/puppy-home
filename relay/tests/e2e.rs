//! End-to-end: a real relay on a real socket, two real TCP clients.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use puppy_relay::server;

/// A test client: connected, joined, with a buffered line reader.
struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Client {
    fn join(port: u16, room: &str, user: &str) -> Client {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        let mut c = Client { stream, reader };
        c.send(&format!(
            r#"{{"op":"join","room":"{room}","user":"{user}","puppy":"{user}-pup","proto":4}}"#
        ));
        c
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stream, "{line}").unwrap();
        self.stream.flush().unwrap();
    }

    /// Next protocol line (panics on timeout -- tests must never hang).
    fn read_line(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read within 5s");
        assert!(!line.is_empty(), "relay closed the connection");
        line.trim_end().to_string()
    }
}

fn start_relay() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let _ = server::run(listener);
    });
    port
}

#[test]
fn two_members_chat_through_a_room() {
    let port = start_relay();

    let mut alice = Client::join(port, "pack-room", "alice");
    let joined = alice.read_line();
    assert!(joined.contains(r#""event":"joined""#) && joined.contains("alice"));

    let mut bob = Client::join(port, "pack-room", "bob");
    let joined_b = bob.read_line();
    assert!(joined_b.contains("alice") && joined_b.contains("bob"));
    assert!(
        joined_b.contains("alice-pup"),
        "roster carries puppy names: {joined_b}"
    );
    assert!(
        joined_b.contains(r#""host":true"#) && joined_b.contains(r#""color":"#),
        "members carry host + relay-assigned colors: {joined_b}"
    );

    // Alice hears bob arrive (announce + system feed narration).
    let seen = alice.read_line();
    assert!(seen.contains(r#""event":"member_joined""#) && seen.contains("bob"));
    assert!(seen.contains("bob-pup"));
    let narrated = alice.read_line();
    assert!(narrated.contains(r#""kind":"system""#) && narrated.contains("joined the den"));

    // Chat reaches both as a Human feed entry, stamped with the relay-known
    // sender + a room-monotonic seq.
    alice.send(r#"{"op":"chat","text":"hello den"}"#);
    for client in [&mut alice, &mut bob] {
        let chat = client.read_line();
        assert!(chat.contains(r#""event":"feed""#) && chat.contains(r#""kind":"human""#));
        assert!(chat.contains(r#""user":"alice""#) && chat.contains("hello den"));
    }

    // Presence flips fan out both ways.
    bob.send(r#"{"op":"presence","presence":"idle"}"#);
    for client in [&mut alice, &mut bob] {
        let p = client.read_line();
        assert!(p.contains(r#""event":"presence""#) && p.contains(r#""presence":"idle""#));
        assert!(p.contains(r#""user":"bob""#));
    }

    // Roster summaries fan out, stamped with the sender.
    alice.send(
        r#"{"op":"roster","agents":[{"puppy":"Rex","agent":"code-puppy","model":"opus","state":"tool","verb":"edit","file":"src/x.rs","dir":"demo","tps":42.0,"added":3,"removed":1}]}"#,
    );
    for client in [&mut alice, &mut bob] {
        let r = client.read_line();
        assert!(r.contains(r#""event":"roster""#) && r.contains(r#""from":"alice""#));
        assert!(r.contains("src/x.rs") && r.contains(r#""tps":42.0"#));
    }

    // Activity fans out the same way.
    bob.send(r#"{"op":"activity","kind":"tool","detail":"edit_file src/x.rs"}"#);
    for client in [&mut alice, &mut bob] {
        let act = client.read_line();
        assert!(act.contains(r#""event":"activity""#) && act.contains("edit_file src/x.rs"));
    }

    // Bob leaves; alice is told (announce + narration).
    bob.send(r#"{"op":"leave"}"#);
    let left = alice.read_line();
    assert!(left.contains(r#""event":"member_left""#) && left.contains("bob"));
    let narrated = alice.read_line();
    assert!(narrated.contains(r#""kind":"system""#) && narrated.contains("left the den"));
}

/// Feed ordering: human + puppy entries interleave with strictly increasing
/// `seq`, and puppy entries carry their addressing + review badge.
#[test]
fn den_feed_orders_and_carries_puppy_messages() {
    let port = start_relay();
    let mut alice = Client::join(port, "feed-room", "alice");
    alice.read_line(); // joined

    alice.send(r#"{"op":"chat","text":"one"}"#);
    alice.send(
        r#"{"op":"puppy_msg","puppy":"Rex","to_puppy":"Biscuit","review":true,"text":"dedupe on event.id"}"#,
    );
    let first = alice.read_line();
    let second = alice.read_line();
    assert!(first.contains(r#""kind":"human""#) && first.contains("one"));
    assert!(second.contains(r#""kind":"puppy""#) && second.contains(r#""puppy":"Rex""#));
    assert!(second.contains(r#""to_puppy":"Biscuit""#) && second.contains(r#""review":true"#));
    let seq = |line: &str| -> u64 {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        v["entry"]["seq"].as_u64().unwrap()
    };
    assert!(seq(&second) > seq(&first), "server ordering is monotonic");
}

/// Kanban lives on the relay: create/move broadcast the board, and a late
/// joiner receives the whole den state in their join snapshot.
#[test]
fn kanban_and_plans_snapshot_on_late_join() {
    let port = start_relay();
    let mut alice = Client::join(port, "board-room", "alice");
    alice.read_line(); // joined

    alice.send(
        r#"{"op":"task_create","title":"Webhooks","column":"backlog","owner":"alice","plan":true}"#,
    );
    let tasks = alice.read_line();
    assert!(tasks.contains(r#""event":"tasks""#) && tasks.contains("Webhooks"));
    alice.send(r#"{"op":"task_move","id":1,"column":"in_progress"}"#);
    let tasks = alice.read_line();
    assert!(tasks.contains(r#""in_progress""#));

    alice.send(r#"{"op":"plan_share","puppy":"Rex","markdown":"- [x] verify\n- [ ] refunds"}"#);
    let plans = alice.read_line();
    assert!(plans.contains(r#""event":"plans""#) && plans.contains("refunds"));
    let narrated = alice.read_line();
    assert!(narrated.contains("shared plans.md"));

    // The late joiner's snapshot has it all: members, board, plans, feed tail.
    let mut bob = Client::join(port, "board-room", "bob");
    let joined = bob.read_line();
    assert!(joined.contains(r#""event":"joined""#));
    assert!(
        joined.contains("Webhooks") && joined.contains(r#""in_progress""#),
        "snapshot carries the board: {joined}"
    );
    assert!(
        joined.contains(r#""puppy":"Rex""#) && joined.contains("refunds"),
        "snapshot carries shared plans: {joined}"
    );
    assert!(
        joined.contains("shared plans.md"),
        "snapshot carries the feed tail: {joined}"
    );
}

#[test]
fn rooms_do_not_leak_into_each_other() {
    let port = start_relay();
    let mut a = Client::join(port, "room-one", "alice");
    a.read_line(); // joined
    let mut b = Client::join(port, "room-two", "bob");
    b.read_line(); // joined

    a.send(r#"{"op":"chat","text":"secret"}"#);
    let echo = a.read_line();
    assert!(echo.contains(r#""event":"feed""#) && echo.contains("secret"));

    // Bob must hear nothing: send him a chat of his own and assert the FIRST
    // thing he receives is that (not alice's message).
    b.send(r#"{"op":"chat","text":"bob-only"}"#);
    let first = b.read_line();
    assert!(
        first.contains("bob-only"),
        "cross-room leak: bob saw {first:?}"
    );
}

/// The agent-helper path: one-shot connections that claim/list without joining.
#[test]
fn one_shot_claims_work_without_joining() {
    let port = start_relay();
    let mut alice = Client::join(port, "work-room", "alice");
    alice.read_line(); // joined

    // One-shot claim (what `pack_helper.py claim` sends).
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut w = stream.try_clone().unwrap();
    writeln!(
        w,
        r#"{{"op":"claim","room":"work-room","user":"alice","puppy":"Rex","path":"src/auth.rs","note":"refactor"}}"#
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(line.contains(r#""event":"claim_result""#) && line.contains(r#""ok":true"#));

    // The joined member sees the claims broadcast.
    let seen = alice.read_line();
    assert!(seen.contains(r#""event":"claims""#) && seen.contains("src/auth.rs"));

    // A rival one-shot claim is refused and names the holder.
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut w = stream.try_clone().unwrap();
    writeln!(
        w,
        r#"{{"op":"claim","room":"work-room","user":"bob","path":"src/auth.rs"}}"#
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(line.contains(r#""ok":false"#) && line.contains("alice"));
}

#[test]
fn join_is_required_and_versioned() {
    let port = start_relay();

    // Not-a-join first message is rejected.
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut w = stream.try_clone().unwrap();
    writeln!(w, r#"{{"op":"chat","text":"hi"}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(line.contains(r#""event":"error""#) && line.contains("join"));

    // A future-versioned client is told about the mismatch.
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut w = stream.try_clone().unwrap();
    writeln!(w, r#"{{"op":"join","room":"r","user":"u","proto":99}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(line.contains("protocol mismatch"));
}
