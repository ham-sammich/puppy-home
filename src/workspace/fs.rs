//! Filesystem access for a workspace's *files* (the IDE tree + editor).
//!
//! Phase A, increment 1: introduce a trait so the file tree and editor stop
//! calling `std::fs` directly. Today the only implementation is [`LocalFs`]
//! (thin wrappers over `std::fs`), so behaviour is unchanged; a later
//! increment adds a remote implementation that routes these calls over the
//! sidecar protocol to a folder on another host.
//!
//! Scope note: this abstracts only workspace-relative file operations. Host
//! config/state (session.json, themes, plugin install, sidecar extraction)
//! always stays on the local machine and deliberately keeps using `std::fs`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One entry from a directory listing. The caller filters/sorts; this is just
/// the raw (name, full path, is-directory) triple the tree needs.
#[derive(Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Abstract access to a workspace's files. Methods mirror the `std::fs` calls
/// the IDE makes today; a remote implementation will route them over the
/// protocol. `Send + Sync` so a workspace can hold one behind an `Arc`.
pub trait WorkspaceFs: Send + Sync {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
    /// List a directory's immediate children (unsorted, unfiltered).
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Create an empty file (truncating if it somehow exists).
    fn create_file(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
}

/// The local-disk implementation: thin wrappers over `std::fs`.
pub struct LocalFs;

impl WorkspaceFs for LocalFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            // `file_type` avoids an extra stat where the OS already knows.
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                is_dir,
            });
        }
        Ok(out)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        std::fs::File::create(path).map(|_| ())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// How long a cached directory listing stays fresh. The tree re-renders every
/// frame; a couple of seconds of staleness is invisible, skipping thousands of
/// directory enumerations per minute (NTFS + Defender make those genuinely
/// expensive on Windows).
const TREE_CACHE_TTL: Duration = Duration::from_millis(2000);

/// A read-dir TTL cache over [`LocalFs`], for the per-frame file tree.
/// Mutations made *through this fs* (the tree's create/rename/delete) drop the
/// cache immediately, so the UI updates in the same frame; outside changes
/// (the agent, the user's editor) appear within the TTL.
///
/// The sidecar-RPC remote fs keeps its own event-driven cache -- wrapping it
/// here would add pointless refetches. The SSH-fallback fs has no cache of
/// its own, so it DOES ride this wrapper (each listing is an ssh round-trip
/// -- the TTL matters even more there than on NTFS).
pub struct CachedFs {
    inner: Box<dyn WorkspaceFs>,
    cache: Mutex<HashMap<PathBuf, (Instant, Vec<DirEntry>)>>,
}

impl CachedFs {
    pub fn new(inner: impl WorkspaceFs + 'static) -> Self {
        CachedFs {
            inner: Box::new(inner),
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn invalidate(&self) {
        self.cache.lock().unwrap().clear();
    }
}

impl WorkspaceFs for CachedFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        // Content writes don't change listings; no invalidation needed.
        self.inner.write(path, contents)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        {
            let cache = self.cache.lock().unwrap();
            if let Some((at, entries)) = cache.get(path)
                && at.elapsed() < TREE_CACHE_TTL
            {
                return Ok(entries.clone());
            }
        }
        let entries = self.inner.read_dir(path)?;
        self.cache
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), (Instant::now(), entries.clone()));
        Ok(entries)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let r = self.inner.create_dir(path);
        self.invalidate();
        r
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        let r = self.inner.create_file(path);
        self.invalidate();
        r
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let r = self.inner.remove_file(path);
        self.invalidate();
        r
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        let r = self.inner.remove_dir_all(path);
        self.invalidate();
        r
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let r = self.inner.rename(from, to);
        self.invalidate();
        r
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_fs_round_trips_a_file_and_lists_it() {
        let dir = std::env::temp_dir().join(format!("ph_fs_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fs = LocalFs;
        fs.create_dir(&dir).unwrap();
        assert!(dir.is_dir());

        let file = dir.join("hello.txt");
        fs.write(&file, b"hi there").unwrap();
        assert!(fs.exists(&file));
        assert!(!file.is_dir());
        assert_eq!(fs.read_to_string(&file).unwrap(), "hi there");

        let sub = dir.join("sub");
        fs.create_dir(&sub).unwrap();
        let listing = fs.read_dir(&dir).unwrap();
        assert_eq!(listing.len(), 2);
        assert!(listing.iter().any(|e| e.name == "hello.txt" && !e.is_dir));
        assert!(listing.iter().any(|e| e.name == "sub" && e.is_dir));

        // rename then delete
        let renamed = dir.join("bye.txt");
        fs.rename(&file, &renamed).unwrap();
        assert!(!fs.exists(&file));
        assert!(fs.exists(&renamed));
        fs.remove_file(&renamed).unwrap();
        assert!(!fs.exists(&renamed));

        fs.remove_dir_all(&dir).unwrap();
        assert!(!fs.exists(&dir));
    }

    #[test]
    fn cached_fs_serves_stale_within_ttl_but_sees_own_mutations() {
        let dir = std::env::temp_dir().join(format!("ph_cfs_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = CachedFs::new(LocalFs);

        assert!(fs.read_dir(&dir).unwrap().is_empty());
        // An OUTSIDE change is invisible while the cache is fresh...
        std::fs::write(dir.join("sneaky.txt"), b"x").unwrap();
        assert!(
            fs.read_dir(&dir).unwrap().is_empty(),
            "cache should serve the fresh listing"
        );
        // ...but a mutation THROUGH the fs invalidates immediately and the
        // next listing reflects everything (including the outside change).
        fs.create_file(&dir.join("mine.txt")).unwrap();
        let names: Vec<String> = fs
            .read_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"mine.txt".to_string()));
        assert!(names.contains(&"sneaky.txt".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
