//! TCP front door: one thread per connection, line-in -> hub -> queued line-out.
//!
//! Each connection gets an outbound `mpsc` queue drained by its own writer
//! thread, so one slow client can never stall a room broadcast (the hub only
//! ever does non-blocking channel sends).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;

use crate::hub::Hub;
use crate::protocol::{ClientMsg, PROTO_VERSION, ServerMsg};

/// Accept loop. Runs until the listener errors out (i.e. effectively forever).
pub fn run(listener: TcpListener) -> std::io::Result<()> {
    let hub = Arc::new(Hub::new());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let hub = hub.clone();
        std::thread::Builder::new()
            .name("relay-conn".into())
            .spawn(move || handle_conn(hub, stream))
            .ok();
    }
    Ok(())
}

fn error_line(message: &str) -> String {
    serde_json::to_string(&ServerMsg::Error {
        message: message.to_string(),
    })
    .expect("ServerMsg serializes")
}

/// Write a single line directly (pre-join, before the writer thread exists).
fn send_direct(stream: &mut TcpStream, line: &str) {
    let _ = writeln!(stream, "{line}");
    let _ = stream.flush();
}

fn handle_conn(hub: Arc<Hub>, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let Ok(mut write_half) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    // The first line must be a valid, version-compatible join.
    let first = lines
        .next()
        .and_then(|l| l.ok())
        .and_then(|l| serde_json::from_str::<ClientMsg>(&l).ok());
    let Some(ClientMsg::Join {
        room,
        user,
        puppy,
        proto,
    }) = first
    else {
        send_direct(&mut write_half, &error_line("first message must be a join"));
        return;
    };
    if proto != 0 && proto != PROTO_VERSION {
        send_direct(
            &mut write_half,
            &error_line(&format!(
                "protocol mismatch: relay speaks v{PROTO_VERSION}, client sent v{proto}"
            )),
        );
        return;
    }
    let (room, user) = (room.trim().to_string(), user.trim().to_string());
    if room.is_empty() || user.is_empty() {
        send_direct(&mut write_half, &error_line("room and user are required"));
        return;
    }

    // Outbound queue -> writer thread.
    let (tx, rx) = mpsc::channel::<String>();
    let writer = std::thread::Builder::new()
        .name("relay-write".into())
        .spawn(move || {
            for line in rx {
                if writeln!(write_half, "{line}").is_err() {
                    break;
                }
                let _ = write_half.flush();
            }
        })
        .ok();

    let (id, members) = hub.join(&room, &user, puppy.trim(), tx.clone());
    let _ = tx.send(
        serde_json::to_string(&ServerMsg::Joined {
            room: room.clone(),
            members,
        })
        .expect("ServerMsg serializes"),
    );

    for line in lines {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ClientMsg>(&line) {
            Ok(ClientMsg::Chat { text }) => hub.chat(id, &text),
            Ok(ClientMsg::Activity { kind, detail }) => hub.activity(id, &kind, &detail),
            Ok(ClientMsg::Leave) => break,
            Ok(ClientMsg::Join { .. }) => {
                let _ = tx.send(error_line("already joined"));
            }
            Err(e) => {
                let _ = tx.send(error_line(&format!("bad message: {e}")));
            }
        }
    }

    // Disconnect: tell the room, then let the writer drain and exit (the hub
    // dropped its sender in `leave`, and ours goes out of scope here).
    hub.leave(id);
    drop(tx);
    if let Some(w) = writer {
        let _ = w.join();
    }
}
