//! Puppy Pack client: a TCP line-JSON connection to a `puppy-relay`.
//!
//! Reuses the relay crate's wire types (`ClientMsg`/`ServerMsg`), so there is
//! exactly one protocol definition for both ends. A reader thread parses
//! incoming lines into [`PackEvent`]s on a channel and wakes the UI -- the same
//! events-in-over-a-channel pattern every other backend in the app uses.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use puppy_relay::protocol::{ClientMsg, PROTO_VERSION, ServerMsg};

use crate::waker::UiWaker;

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

        match rx
            .recv_timeout(Duration::from_secs(5))
            .expect("joined event")
        {
            PackEvent::Msg(ServerMsg::Joined { room, members }) => {
                assert_eq!(room, "test-room");
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].user, "tester");
                assert_eq!(members[0].puppy, "Rex");
            }
            _ => panic!("expected joined first"),
        }

        client.chat("woof");
        match rx.recv_timeout(Duration::from_secs(5)).expect("chat event") {
            PackEvent::Msg(ServerMsg::Chat { from, text, .. }) => {
                assert_eq!(from, "tester");
                assert_eq!(text, "woof");
            }
            _ => panic!("expected the chat back"),
        }
    }
}
