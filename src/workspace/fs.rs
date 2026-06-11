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

use std::io;
use std::path::{Path, PathBuf};

/// One entry from a directory listing. The caller filters/sorts; this is just
/// the raw (name, full path, is-directory) triple the tree needs.
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
    fn is_dir(&self, path: &Path) -> bool;
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

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
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
        assert!(fs.is_dir(&dir));

        let file = dir.join("hello.txt");
        fs.write(&file, b"hi there").unwrap();
        assert!(fs.exists(&file));
        assert!(!fs.is_dir(&file));
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
}
