//! File-tree mutations (create / rename / delete) and their confirm modals,
//! split out of `editor.rs`. All are `impl Workspace` methods and route their
//! filesystem changes through `self.fs` (the [`WorkspaceFs`](super::fs)).

use std::path::{Path, PathBuf};

use super::Workspace;
use super::state::EditorItem;

impl Workspace {
    /// Remove a file/folder from disk and close any editor tabs/buffers for it.
    pub(crate) fn delete_path(&mut self, path: &Path, is_dir: bool) -> Result<(), String> {
        if is_dir {
            self.fs.remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            self.fs.remove_file(path).map_err(|e| e.to_string())?;
        }
        // Forget any open buffers / editor tabs for the path (or its children).
        self.open_files
            .retain(|p, _| !(p == path || p.starts_with(path)));
        self.editor_open.retain(|it| match it {
            EditorItem::File(p) => !(p == path || p.starts_with(path)),
            _ => true,
        });
        if self.editor_active >= self.editor_open.len() {
            self.editor_active = self.editor_open.len().saturating_sub(1);
        }
        // Nudge the git/tree state to refresh now that the tree changed.
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }

    pub(crate) fn perform_rename(&mut self, path: &Path, new_name: &str) -> Result<(), String> {
        let dest = sibling_path(path.parent(), new_name)?;
        if self.fs.exists(&dest) {
            return Err(format!(
                "\u{201c}{}\u{201d} already exists",
                new_name.trim()
            ));
        }
        self.fs.rename(path, &dest).map_err(|e| e.to_string())?;
        // Keep open buffers / editor tabs pointing at the renamed path(s).
        let taken = std::mem::take(&mut self.open_files);
        for (p, buf) in taken {
            self.open_files.insert(remap(&p, path, &dest), buf);
        }
        for item in &mut self.editor_open {
            if let EditorItem::File(p) = item {
                *p = remap(p, path, &dest);
            }
        }
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }

    pub(crate) fn perform_new(
        &mut self,
        parent: &Path,
        is_dir: bool,
        name: &str,
    ) -> Result<(), String> {
        let dest = sibling_path(Some(parent), name)?;
        if self.fs.exists(&dest) {
            return Err(format!("\u{201c}{}\u{201d} already exists", name.trim()));
        }
        if is_dir {
            self.fs.create_dir(&dest).map_err(|e| e.to_string())?;
        } else {
            self.fs.create_file(&dest).map_err(|e| e.to_string())?;
            self.open_editor_file(dest.clone());
        }
        self.git_refresh_at = std::time::Instant::now();
        Ok(())
    }
}

/// Validate a bare entry name (no separators) and join it onto `parent`.
fn sibling_path(parent: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name can't be empty".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Name can't contain path separators".into());
    }
    let parent = parent.ok_or("No parent directory")?;
    Ok(parent.join(name))
}

/// Rewrite `p` if it equals `old` or lives under it, mapping that prefix to
/// `new` (used to keep open buffers/tabs valid across a rename).
fn remap(p: &Path, old: &Path, new: &Path) -> PathBuf {
    if p == old {
        new.to_path_buf()
    } else if let Ok(rel) = p.strip_prefix(old) {
        new.join(rel)
    } else {
        p.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::{remap, sibling_path};
    use std::path::{Path, PathBuf};

    #[test]
    fn sibling_path_validates() {
        assert!(sibling_path(Some(Path::new("/a")), "").is_err());
        assert!(sibling_path(Some(Path::new("/a")), "a/b").is_err());
        assert!(sibling_path(Some(Path::new("/a")), "a\\b").is_err());
        assert_eq!(
            sibling_path(Some(Path::new("/a")), " notes.txt ").unwrap(),
            PathBuf::from("/a/notes.txt")
        );
    }

    #[test]
    fn remap_rewrites_old_prefix() {
        let old = Path::new("/a/old");
        let new = Path::new("/a/new");
        assert_eq!(
            remap(Path::new("/a/old"), old, new),
            PathBuf::from("/a/new")
        );
        assert_eq!(
            remap(Path::new("/a/old/x.rs"), old, new),
            PathBuf::from("/a/new/x.rs")
        );
        assert_eq!(
            remap(Path::new("/a/other"), old, new),
            PathBuf::from("/a/other")
        );
    }
}
