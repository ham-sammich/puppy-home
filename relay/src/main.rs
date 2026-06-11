//! `puppy-relay` -- run a Puppy Pack relay.
//!
//! Usage: `puppy-relay [port]` (default 9220, or `PUPPY_RELAY_PORT`).
//! No accounts, no persistence: rooms exist while members are connected, and
//! the room code is the shared secret.

use std::net::TcpListener;

fn main() {
    let port = std::env::args()
        .nth(1)
        .and_then(|p| p.parse::<u16>().ok())
        .or_else(|| {
            std::env::var("PUPPY_RELAY_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .unwrap_or(9220);

    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("puppy-relay: failed to bind 0.0.0.0:{port}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "puppy-relay v{} listening on 0.0.0.0:{port}",
        env!("CARGO_PKG_VERSION")
    );
    if let Err(e) = puppy_relay::server::run(listener) {
        eprintln!("puppy-relay: {e}");
        std::process::exit(1);
    }
}
