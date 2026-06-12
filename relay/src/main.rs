//! `puppy-relay` -- run a Puppy Pack relay.
//!
//! Usage: `puppy-relay [port]` (default 9220, or `PUPPY_RELAY_PORT`).
//! No accounts, no persistence: rooms exist while members are connected, and
//! the room code is the shared secret.

use std::net::TcpListener;

/// `PUPPY_RELAY_WATCH_PID=<pid>`: exit when that process dies. Set by
/// puppy-home's in-app "Host a Den" so a killed app (SIGKILL / killed
/// from Task Manager — Drop never runs) can't leave an orphan relay.
/// Unix: `kill -0` probe every 3s; Windows: `tasklist /FI "PID eq N"`
/// (by construction — G3 gate item verifies).
fn spawn_parent_watchdog() {
    let Ok(pid) = std::env::var("PUPPY_RELAY_WATCH_PID") else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            #[cfg(unix)]
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(true);
            #[cfg(windows)]
            let alive = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                // CSV rows quote each field; a live pid appears as ","N",".
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
                .unwrap_or(true);
            #[cfg(not(any(unix, windows)))]
            let alive = true;
            if !alive {
                eprintln!("puppy-relay: parent {pid} gone, exiting");
                std::process::exit(0);
            }
        }
    });
}

fn main() {
    spawn_parent_watchdog();
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
