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
            r#"{{"op":"join","room":"{room}","user":"{user}","puppy":"{user}-pup","proto":3}}"#
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

    // Alice hears bob arrive (with his puppy).
    let seen = alice.read_line();
    assert!(seen.contains(r#""event":"member_joined""#) && seen.contains("bob"));
    assert!(seen.contains("bob-pup"));

    // Chat reaches both, stamped with the relay-known sender.
    alice.send(r#"{"op":"chat","text":"hello pack"}"#);
    for client in [&mut alice, &mut bob] {
        let chat = client.read_line();
        assert!(chat.contains(r#""from":"alice""#) && chat.contains("hello pack"));
    }

    // Activity fans out the same way.
    bob.send(r#"{"op":"activity","kind":"tool","detail":"edit_file src/x.rs"}"#);
    for client in [&mut alice, &mut bob] {
        let act = client.read_line();
        assert!(act.contains(r#""event":"activity""#) && act.contains("edit_file src/x.rs"));
    }

    // Bob leaves; alice is told.
    bob.send(r#"{"op":"leave"}"#);
    let left = alice.read_line();
    assert!(left.contains(r#""event":"member_left""#) && left.contains("bob"));
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
    assert!(echo.contains("secret"));

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
