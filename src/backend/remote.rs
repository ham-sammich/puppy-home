//! Remote filesystem access for SSH-hosted workspaces.
//!
//! A remote workspace's sidecar runs on the far host, so the file tree + editor
//! read files by RPC over the *same* stdio channel the chat uses: the GUI sends
//! `fs_list_dir` / `fs_read_file` ops and the sidecar replies with `fs_result`
//! lines. The stdout reader thread routes those replies here (via
//! [`RemoteState::handle_result`]) rather than through the `UiEvent` path -- a
//! blocking `read_to_string` runs on the UI thread, so its reply must be
//! delivered off that thread or we'd deadlock.
//!
//! Directory listings are cached and filled asynchronously (the tree renders
//! every frame, so listing must never block); file reads block on a one-shot
//! channel with a timeout. Writes/mutations aren't supported yet -- a remote
//! workspace is read-only for now (browse + view).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ChildStdin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use serde_json::{Value, json};

use crate::workspace::fs::{DirEntry, WorkspaceFs};

/// How long a blocking remote file read waits before giving up.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Cache state for one directory listing.
enum DirState {
    Pending,
    Ready(Vec<(String, bool)>), // (name, is_dir)
    Failed,
}

/// A pending RPC awaiting its `fs_result` reply.
enum Pending {
    ListDir(PathBuf),
    ReadFile(SyncSender<Result<String, String>>),
}

/// Shared state behind a [`RemoteFs`]: the sidecar stdin (to send ops), the
/// listing cache, and the in-flight request table. Filled by the stdout reader.
pub struct RemoteState {
    stdin: Arc<Mutex<ChildStdin>>,
    ctx: egui::Context,
    next_id: AtomicU64,
    dirs: Mutex<HashMap<PathBuf, DirState>>,
    pending: Mutex<HashMap<u64, Pending>>,
}

impl RemoteState {
    pub fn new(stdin: Arc<Mutex<ChildStdin>>, ctx: egui::Context) -> Arc<Self> {
        Arc::new(RemoteState {
            stdin,
            ctx,
            next_id: AtomicU64::new(1),
            dirs: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        })
    }

    fn send(&self, obj: Value) {
        if let Ok(mut stdin) = self.stdin.lock() {
            use std::io::Write;
            let _ = writeln!(stdin, "{obj}");
            let _ = stdin.flush();
        }
    }

    /// Dispatch one `fs_result` line. Called from the stdout reader thread, so
    /// it never runs on (and never blocks) the UI thread.
    pub fn handle_result(&self, val: &Value) {
        let Some(id) = val.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(pending) = self.pending.lock().unwrap().remove(&id) else {
            return;
        };
        let ok = val.get("ok").and_then(Value::as_bool).unwrap_or(false);
        match pending {
            Pending::ListDir(path) => {
                let state = if ok {
                    DirState::Ready(parse_entries(val))
                } else {
                    DirState::Failed
                };
                self.dirs.lock().unwrap().insert(path, state);
                self.ctx.request_repaint();
            }
            Pending::ReadFile(tx) => {
                let result = if ok {
                    Ok(val
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string())
                } else {
                    Err(val
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("remote read failed")
                        .to_string())
                };
                let _ = tx.send(result);
            }
        }
    }

    /// Non-blocking: return the cached listing (empty while it loads), kicking
    /// off a fetch on a cache miss. The reply triggers a repaint to refill.
    fn list_dir(&self, path: &Path) -> Vec<DirEntry> {
        {
            let dirs = self.dirs.lock().unwrap();
            match dirs.get(path) {
                Some(DirState::Ready(list)) => {
                    return list
                        .iter()
                        .map(|(name, is_dir)| DirEntry {
                            name: name.clone(),
                            path: path.join(name),
                            is_dir: *is_dir,
                        })
                        .collect();
                }
                // Pending or failed: show nothing this frame.
                Some(_) => return Vec::new(),
                None => {}
            }
        }
        self.dirs
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), DirState::Pending);
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending::ListDir(path.to_path_buf()));
        self.send(json!({ "op": "fs_list_dir", "id": id, "path": path.to_string_lossy() }));
        Vec::new()
    }

    /// Blocking (off the UI thread is fine -- the reply lands on the reader
    /// thread): request a file's contents and wait, bounded by `READ_TIMEOUT`.
    fn read_file(&self, path: &Path) -> io::Result<String> {
        let (tx, rx) = sync_channel(1);
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending::ReadFile(tx));
        self.send(json!({ "op": "fs_read_file", "id": id, "path": path.to_string_lossy() }));
        match rx.recv_timeout(READ_TIMEOUT) {
            Ok(Ok(content)) => Ok(content),
            Ok(Err(e)) => Err(io::Error::other(e)),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(io::Error::other("remote read timed out"))
            }
        }
    }
}

/// Pull `{name, is_dir}` entries out of an `fs_result` listing reply.
fn parse_entries(val: &Value) -> Vec<(String, bool)> {
    let Some(arr) = val.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .map(|e| {
            let name = e
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let is_dir = e.get("is_dir").and_then(Value::as_bool).unwrap_or(false);
            (name, is_dir)
        })
        .collect()
}

/// A [`WorkspaceFs`] backed by a remote sidecar over RPC. Read-only for now;
/// mutations return an error until remote editing lands.
pub struct RemoteFs {
    state: Arc<RemoteState>,
}

impl RemoteFs {
    pub fn new(state: Arc<RemoteState>) -> Self {
        RemoteFs { state }
    }
}

fn unsupported() -> io::Error {
    io::Error::other("editing remote files isn't supported yet")
}

impl WorkspaceFs for RemoteFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.state.read_file(path)
    }
    fn write(&self, _path: &Path, _contents: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        Ok(self.state.list_dir(path))
    }
    fn create_dir(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn create_file(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn remove_file(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn remove_dir_all(&self, _path: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn rename(&self, _from: &Path, _to: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    fn exists(&self, _path: &Path) -> bool {
        false
    }
    fn is_dir(&self, _path: &Path) -> bool {
        false
    }
}
