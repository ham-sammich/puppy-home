//! Live PTY smoke test mirroring Terminal::spawn — KEEPS THE SLAVE ALIVE.
//! If this produces shell output, the "dropped slave tears down ConPTY" theory
//! is confirmed as the typing/blank-terminal bug.
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[test]
fn live_pty_produces_output_with_slave_kept_alive() {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = if cfg!(windows) {
        CommandBuilder::new("cmd.exe")
    } else {
        CommandBuilder::new("/bin/sh")
    };
    if !cfg!(windows) {
        cmd.arg("-i");
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    // NOTE: do NOT drop pair.slave — keep it alive (the fix).
    let _slave = pair.slave;

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));

    let raw = Arc::new(Mutex::new(Vec::<u8>::new()));
    let r2 = raw.clone();
    let w2 = writer.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    r2.lock().unwrap().extend_from_slice(chunk);
                    // Answer DSR cursor-position queries (ESC[6n) so conhost proceeds.
                    if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                        let mut w = w2.lock().unwrap();
                        let _ = w.write_all(b"\x1b[1;1R");
                        let _ = w.flush();
                    }
                }
            }
        }
    });

    thread::sleep(Duration::from_millis(1500));
    writer
        .lock()
        .unwrap()
        .write_all(b"echo PTYHELLO123\r\n")
        .unwrap();
    writer.lock().unwrap().flush().unwrap();
    thread::sleep(Duration::from_millis(2000));

    let bytes = raw.lock().unwrap().clone();
    let captured = String::from_utf8_lossy(&bytes).to_string();
    eprintln!("child alive? {:?}", child.try_wait());
    let _ = writer.lock().unwrap().write_all(b"exit\r\n");
    let _ = child.kill();

    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    eprintln!(
        "=== captured {} bytes: [{}] ===",
        bytes.len(),
        hex.join(" ")
    );
    eprintln!("=== as text ===\n{captured}\n=== end ===");
    assert!(
        captured.contains("PTYHELLO123"),
        "no shell output captured (len={})",
        captured.len()
    );
}
