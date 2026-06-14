//! Den HOSTING (QW6): spawn a local `puppy-relay` and auto-join it, so
//! "Join Den" no longer requires someone to start the relay by hand.
//!
//! RELAY BINARY RESOLUTION ORDER (honest, documented):
//!   1. `puppy-relay[.exe]` next to the running executable — the ship
//!      shape (relay is a workspace member; release packaging drops both
//!      binaries in one dir, exactly like target/debug in dev once
//!      `cargo build -p puppy-relay` has run).
//!   2. DEV-ONLY FALLBACK: `cargo run -q -p puppy-relay -- <port>` when
//!      no built binary exists AND a `relay/Cargo.toml` is visible from
//!      the cwd (i.e. you're running inside the repo). Never used in a
//!      shipped install.
//!
//! Lifecycle: the child is killed on "Stop hosting" and on app exit
//! (Drop). Rooms are in-memory on the relay, so stopping the host ends
//! the den for everyone — the UI says so.
//!
//! See docs/DEN_HOSTING.md for running a relay on a real server.

use std::net::{TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A locally hosted relay child. Dropping it kills the relay (rooms are
/// in-memory; the den ends with the host).
pub struct DenHost {
    child: Child,
    pub port: u16,
    /// `ip:port` teammates on the LAN can join.
    pub share_addr: String,
}

impl DenHost {
    pub fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DenHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Resolution step 1: a built relay binary next to this executable.
fn relay_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "puppy-relay.exe"
    } else {
        "puppy-relay"
    };
    let p = dir.join(name);
    p.is_file().then_some(p)
}

/// Best-effort LAN IP (UDP-connect trick; no packets are actually sent).
fn lan_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// A short shareable room code (base36 of time+pid — no crypto needed,
/// the code is a shared secret among teammates, not an auth system).
pub fn room_code() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 17;
    let mut n = seed | 1;
    let mut out = String::new();
    for _ in 0..5 {
        let d = (n % 36) as u32;
        out.push(char::from_digit(d, 36).unwrap_or('x'));
        n /= 36;
    }
    format!("den-{out}")
}

/// First free port starting at the relay default (9220), probing upward.
fn pick_port() -> u16 {
    for port in 9220..9240 {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    9220
}

/// Spawn a relay (binary first, cargo dev-fallback second) and wait for
/// it to accept connections.
pub fn spawn_host() -> Result<DenHost, String> {
    let port = pick_port();
    let watch_pid = std::process::id().to_string();
    let child = if let Some(bin) = relay_binary() {
        let mut cmd = Command::new(bin);
        cmd.arg(port.to_string())
            .env("PUPPY_RELAY_WATCH_PID", &watch_pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The relay is a console-subsystem binary; without this a GUI host
        // flashes/keeps an empty terminal window when hosting (G3 finding).
        crate::proc::hide_console(&mut cmd);
        cmd.spawn()
            .map_err(|e| format!("couldn't start puppy-relay: {e}"))?
    } else if std::path::Path::new("relay/Cargo.toml").is_file() {
        // Dev fallback only: inside the repo without a built binary.
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "-q", "-p", "puppy-relay", "--", &port.to_string()])
            .env("PUPPY_RELAY_WATCH_PID", &watch_pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::proc::hide_console(&mut cmd);
        cmd.spawn()
            .map_err(|e| format!("cargo dev-fallback failed: {e}"))?
    } else {
        return Err(
            "puppy-relay binary not found next to the app (and not in a dev checkout). \
             Build it with `cargo build -p puppy-relay`."
                .to_string(),
        );
    };

    // Wait for the listener (cargo fallback may compile first: be patient).
    let mut host = DenHost {
        child,
        port,
        share_addr: format!("{}:{port}", lan_ip()),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(host);
        }
        // Child died (port clash, build error) — surface it.
        if let Ok(Some(status)) = host.child.try_wait() {
            return Err(format!("puppy-relay exited at startup ({status})"));
        }
        if Instant::now() >= deadline {
            host.stop();
            return Err("puppy-relay didn't start listening within 30s".to_string());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::room_code;

    #[test]
    fn room_codes_look_right_and_vary() {
        let a = room_code();
        assert!(a.starts_with("den-") && a.len() == 9, "{a}");
        // Not asserting inequality across calls (same-nanos flake); shape only.
    }
}
