//! Remote filesystem access for SSH-hosted workspaces.
//!
//! A remote workspace's sidecar runs on the far host, so the file tree + editor
//! talk to it by RPC over the *same* stdio channel the chat uses: the GUI sends
//! `fs_*` ops and the sidecar replies with `fs_result` lines. The stdout reader
//! thread routes those replies here (via [`RemoteState::handle_result`]) rather
//! than through the `UiEvent` path -- a blocking op runs on the UI thread, so
//! its reply must be delivered off that thread or we'd deadlock.
//!
//! Latency strategy:
//! * **Listings + stats** are cached and filled asynchronously -- the tree
//!   renders every frame, so they must never block. A cache miss kicks off a
//!   request and shows nothing until the reply repaints.
//! * **Reads + mutations** block on a one-shot channel with a timeout. They run
//!   on the UI thread but only in response to a user action (open/save/delete),
//!   so a brief wait is acceptable.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ChildStdin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::waker::UiWaker;
use crate::workspace::fs::{DirEntry, WorkspaceFs};

/// How long a blocking remote op waits before giving up.
const OP_TIMEOUT: Duration = Duration::from_secs(20);

/// Cache state for one directory listing.
enum DirState {
    Pending,
    Ready(Vec<(String, bool)>), // (name, is_dir)
    Failed,
}

/// Cached `exists`/`is_dir` for one path (`None` while the stat is in flight).
#[derive(Clone, Copy)]
struct StatInfo {
    exists: bool,
    is_dir: bool,
}

/// A pending RPC awaiting its `fs_result` reply.
enum Pending {
    /// Fill the listing cache + repaint.
    ListDir(PathBuf),
    /// Fill the stat cache + repaint.
    Stat(PathBuf),
    /// Deliver the whole reply to a blocked caller.
    Reply(SyncSender<Value>),
}

/// Shared state behind a [`RemoteFs`]: the sidecar stdin (to send ops), the
/// listing/stat caches, and the in-flight request table. Filled by the stdout
/// reader thread.
pub struct RemoteState {
    stdin: Arc<Mutex<ChildStdin>>,
    waker: Arc<dyn UiWaker>,
    next_id: AtomicU64,
    dirs: Mutex<HashMap<PathBuf, DirState>>,
    stats: Mutex<HashMap<PathBuf, Option<StatInfo>>>,
    pending: Mutex<HashMap<u64, Pending>>,
}

impl RemoteState {
    pub fn new(stdin: Arc<Mutex<ChildStdin>>, waker: Arc<dyn UiWaker>) -> Arc<Self> {
        Arc::new(RemoteState {
            stdin,
            waker,
            next_id: AtomicU64::new(1),
            dirs: Mutex::new(HashMap::new()),
            stats: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn send(&self, obj: Value) {
        if let Ok(mut stdin) = self.stdin.lock() {
            use std::io::Write;
            let _ = writeln!(stdin, "{obj}");
            let _ = stdin.flush();
        }
    }

    /// Dispatch one `fs_result` line. Runs on the stdout reader thread, so it
    /// never blocks the UI thread.
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
                self.waker.wake();
            }
            Pending::Stat(path) => {
                let info = StatInfo {
                    exists: ok && val.get("exists").and_then(Value::as_bool).unwrap_or(false),
                    is_dir: ok && val.get("is_dir").and_then(Value::as_bool).unwrap_or(false),
                };
                self.stats.lock().unwrap().insert(path, Some(info));
                self.waker.wake();
            }
            Pending::Reply(tx) => {
                let _ = tx.send(val.clone());
            }
        }
    }

    /// Non-blocking: cached listing (empty while it loads); a miss fetches.
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
                Some(_) => return Vec::new(), // pending or failed
                None => {}
            }
        }
        self.dirs
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), DirState::Pending);
        let id = self.next_id();
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending::ListDir(path.to_path_buf()));
        self.send(json!({ "op": "fs_list_dir", "id": id, "path": posix(path) }));
        Vec::new()
    }

    /// Non-blocking: cached stat (defaults to absent while it loads).
    fn stat(&self, path: &Path) -> StatInfo {
        {
            let stats = self.stats.lock().unwrap();
            match stats.get(path) {
                Some(Some(info)) => return *info,
                Some(None) => {
                    return StatInfo {
                        exists: false,
                        is_dir: false,
                    };
                }
                None => {}
            }
        }
        self.stats.lock().unwrap().insert(path.to_path_buf(), None);
        let id = self.next_id();
        self.pending
            .lock()
            .unwrap()
            .insert(id, Pending::Stat(path.to_path_buf()));
        self.send(json!({ "op": "fs_stat", "id": id, "path": posix(path) }));
        StatInfo {
            exists: false,
            is_dir: false,
        }
    }

    /// Blocking: send an op and wait for its reply `Value` (bounded by
    /// `OP_TIMEOUT`). Safe on the UI thread -- the reply lands on the reader.
    fn call(&self, op: &str, extra: Value) -> Result<Value, String> {
        let id = self.next_id();
        let mut obj = match extra {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        obj.insert("op".into(), json!(op));
        obj.insert("id".into(), json!(id));
        let (tx, rx) = sync_channel(1);
        self.pending.lock().unwrap().insert(id, Pending::Reply(tx));
        self.send(Value::Object(obj));
        match rx.recv_timeout(OP_TIMEOUT) {
            Ok(v) => Ok(v),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err("remote operation timed out".to_string())
            }
        }
    }

    /// A blocking op whose reply we only check for ok/error.
    fn call_unit(&self, op: &str, extra: Value) -> io::Result<()> {
        let reply = self.call(op, extra).map_err(io::Error::other)?;
        reply_ok(&reply)
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        let reply = self
            .call("fs_read_file", json!({ "path": posix(path) }))
            .map_err(io::Error::other)?;
        if reply.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            Ok(reply
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string())
        } else {
            Err(io::Error::other(reply_error(&reply)))
        }
    }

    /// Drop a directory from the listing cache (so it re-fetches) + repaint.
    fn invalidate(&self, dir: Option<&Path>) {
        if let Some(dir) = dir {
            self.dirs.lock().unwrap().remove(dir);
            self.stats.lock().unwrap().remove(dir);
        }
        self.waker.wake();
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

/// The error string from a failed reply (or a generic fallback).
fn reply_error(reply: &Value) -> String {
    reply
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("remote operation failed")
        .to_string()
}

/// Turn an ok/error reply into a unit `io::Result`.
fn reply_ok(reply: &Value) -> io::Result<()> {
    if reply.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        Ok(())
    } else {
        Err(io::Error::other(reply_error(reply)))
    }
}

/// A [`WorkspaceFs`] backed by a remote sidecar over RPC.
pub struct RemoteFs {
    state: Arc<RemoteState>,
}

impl RemoteFs {
    pub fn new(state: Arc<RemoteState>) -> Self {
        RemoteFs { state }
    }
}

/// A remote (POSIX) path as the JSON string the sidecar needs. A Windows
/// CLIENT's `PathBuf::join` stitches with '\\', but the remote is POSIX and
/// would take that literally (ENOENT). Normalize so a Windows host can drive a
/// Linux remote. (Backslashes in remote names are vanishingly rare.)
fn posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl WorkspaceFs for RemoteFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.state.read_file(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let text = String::from_utf8_lossy(contents);
        self.state.call_unit(
            "fs_write_file",
            json!({ "path": posix(path), "content": text }),
        )
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        Ok(self.state.list_dir(path))
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let r = self
            .state
            .call_unit("fs_mkdir", json!({ "path": posix(path) }));
        if r.is_ok() {
            self.state.invalidate(path.parent());
        }
        r
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        let r = self
            .state
            .call_unit("fs_create_file", json!({ "path": posix(path) }));
        if r.is_ok() {
            self.state.invalidate(path.parent());
        }
        r
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        // The sidecar auto-detects file vs dir, so both removes map here.
        let r = self
            .state
            .call_unit("fs_remove", json!({ "path": posix(path) }));
        if r.is_ok() {
            self.state.invalidate(path.parent());
        }
        r
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.remove_file(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let r = self.state.call_unit(
            "fs_rename",
            json!({ "from": posix(from), "to": posix(to) }),
        );
        if r.is_ok() {
            self.state.invalidate(from.parent());
            self.state.invalidate(to.parent());
        }
        r
    }

    fn exists(&self, path: &Path) -> bool {
        self.state.stat(path).exists
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.state.stat(path).is_dir
    }
}

// ---------------------------------------------------------------------------
// Remote git: run git on the far host via the sidecar, parse with git.rs.
// ---------------------------------------------------------------------------

/// A [`crate::git::GitRunner`] that runs `git -C <root> ...` on the remote host
/// through the `git_run` RPC.
pub struct RemoteRunner {
    state: Arc<RemoteState>,
    /// The repo root path *on the remote*.
    root: String,
}

impl RemoteRunner {
    fn call_git(&self, args: &[&str], env: &[(&str, &str)]) -> Result<Value, String> {
        let env_map: Map<String, Value> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(v)))
            .collect();
        self.state.call(
            "git_run",
            json!({ "root": self.root, "args": args, "env": Value::Object(env_map) }),
        )
    }
}

impl crate::git::GitRunner for RemoteRunner {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
        let reply = self.call_git(args, env)?;
        let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if ok {
            Ok(reply
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string())
        } else {
            let stderr = reply.get("stderr").and_then(Value::as_str).unwrap_or("");
            let stderr = stderr.trim();
            Err(if stderr.is_empty() {
                format!("git {} failed", args.first().copied().unwrap_or(""))
            } else {
                stderr.to_string()
            })
        }
    }

    fn output(&self, args: &[&str]) -> String {
        self.call_git(args, &[])
            .ok()
            .and_then(|r| r.get("stdout").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default()
    }

    fn read_workfile(&self, rel: &str) -> Option<String> {
        let base = self.root.trim_end_matches('/');
        let full = if base.is_empty() {
            rel.to_string()
        } else {
            format!("{base}/{rel}")
        };
        self.state.read_file(Path::new(&full)).ok()
    }
}

/// A [`crate::git::WorkspaceGit`] backed by git running on the remote host.
pub struct RemoteGit {
    runner: RemoteRunner,
}

impl RemoteGit {
    pub fn new(state: Arc<RemoteState>, root: String) -> Self {
        RemoteGit {
            runner: RemoteRunner { state, root },
        }
    }
}

crate::git::impl_workspace_git!(RemoteGit);
