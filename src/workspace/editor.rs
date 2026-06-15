//! File buffers, editor file tabs, save/dirty/reload, and inline blame.
//!
//! The file-tree mutation ops (create / rename / delete) and their modals live
//! in the sibling `tree_ops` module.

use std::path::Path;
use std::path::PathBuf;

use super::Workspace;
use super::state::{EditorItem, FileBuffer};

/// Split raw file text into an LF-normalized buffer + whether it was CRLF.
/// Mixed endings count as CRLF if ANY `\r\n` is present (Windows default).
pub(crate) fn split_eol(raw: String) -> (String, bool) {
    let crlf = raw.contains("\r\n");
    let content = if crlf { raw.replace("\r\n", "\n") } else { raw };
    (content, crlf)
}

/// Re-apply the original line ending to an LF buffer for writing to disk.
pub(crate) fn restore_eol(content: &str, crlf: bool) -> Vec<u8> {
    if crlf {
        content.replace('\n', "\r\n").into_bytes()
    } else {
        content.as_bytes().to_vec()
    }
}

impl Workspace {
    /// Load a file into an editable buffer (no-op if already open).
    pub fn open_file(&mut self, path: PathBuf) {
        if self.open_files.contains_key(&path) {
            return;
        }
        let buffer = match self.fs.read_to_string(&path) {
            Ok(raw) => {
                // Detect + strip CRLF so the buffer is pure-LF for editing;
                // save_file restores the original style.
                let (content, crlf) = split_eol(raw);
                FileBuffer {
                    saved: content.clone(),
                    content,
                    crlf,
                    dirty: false,
                    load_error: None,
                    save_error: None,
                }
            }
            Err(e) => FileBuffer {
                content: String::new(),
                saved: String::new(),
                crlf: false,
                dirty: false,
                load_error: Some(e.to_string()),
                save_error: None,
            },
        };
        self.open_files.insert(path, buffer);
    }

    /// Whether an open file has unsaved edits (for the tab title marker).
    #[allow(dead_code)] // accessor kept for tab-marker callers; inlined today
    pub fn is_file_dirty(&self, path: &Path) -> bool {
        self.open_files.get(path).map(|b| b.dirty).unwrap_or(false)
    }

    /// Open (or focus) a file in the editor area.
    pub fn open_editor_file(&mut self, path: PathBuf) {
        self.open_file(path.clone());
        let item = EditorItem::File(path);
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    /// Open (or focus) the Changes (diff) tab in the editor area.
    pub fn show_changes(&mut self) {
        match self
            .editor_open
            .iter()
            .position(|t| *t == EditorItem::Changes)
        {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(EditorItem::Changes);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }

    pub(crate) fn close_editor(&mut self, index: usize) {
        if index >= self.editor_open.len() {
            return;
        }
        self.editor_open.remove(index);
        if self.editor_active >= self.editor_open.len() {
            self.editor_active = self.editor_open.len().saturating_sub(1);
        }
    }

    pub(crate) fn focus_or_open(&mut self, item: EditorItem) {
        match self.editor_open.iter().position(|t| *t == item) {
            Some(i) => self.editor_active = i,
            None => {
                self.editor_open.push(item);
                self.editor_active = self.editor_open.len() - 1;
            }
        }
    }
}

/// Map a file extension to a syntect language token for highlighting.
pub(crate) fn language_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rs",
        "py" | "pyw" => "py",
        "toml" => "toml",
        "json" => "json",
        "md" | "markdown" => "md",
        "js" | "mjs" | "cjs" => "js",
        "ts" | "tsx" => "ts",
        "html" | "htm" => "html",
        "css" => "css",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" => "cpp",
        "go" => "go",
        "java" => "java",
        "yaml" | "yml" => "yaml",
        "xml" => "xml",
        "sql" => "sql",
        "rb" => "rb",
        "php" => "php",
        "lua" => "lua",
        _ => "txt",
    }
}

#[cfg(test)]
mod tests {
    use super::language_for;
    use std::path::Path;

    #[test]
    fn language_for_known_extensions() {
        assert_eq!(language_for(Path::new("src/main.rs")), "rs");
        assert_eq!(language_for(Path::new("a.py")), "py");
        assert_eq!(language_for(Path::new("Cargo.toml")), "toml");
        assert_eq!(language_for(Path::new("README.md")), "md");
    }

    #[test]
    fn language_for_unknown_is_txt() {
        assert_eq!(language_for(Path::new("file.xyz")), "txt");
        assert_eq!(language_for(Path::new("noext")), "txt");
    }

    #[test]
    fn crlf_file_roundtrips_with_only_the_edit_changed() {
        let (content, crlf) = super::split_eol("a\r\nb\r\nc\r\n".to_string());
        assert!(crlf);
        assert_eq!(content, "a\nb\nc\n"); // LF in memory
        // edit one line in the LF buffer, then write back
        let edited = content.replace('b', "BEE");
        assert_eq!(super::restore_eol(&edited, crlf), b"a\r\nBEE\r\nc\r\n");
    }

    #[test]
    fn lf_file_stays_lf() {
        let (content, crlf) = super::split_eol("x\ny\n".to_string());
        assert!(!crlf);
        assert_eq!(super::restore_eol(&content, crlf), b"x\ny\n");
    }
}
