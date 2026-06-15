//! Diff data types, parsing (Code Puppy `DiffMessage` + unified git output),
//! marker/style helpers, and low-level diff rendering.

use serde_json::Value;

use crate::backend::BackendMessage;

use super::Workspace;

/// One file change (from Code Puppy's `DiffMessage` or a git diff).
#[derive(Clone)]
pub(crate) struct DiffRecord {
    pub(crate) path: String,
    pub(crate) operation: String,
    pub(crate) adds: usize,
    pub(crate) dels: usize,
    pub(crate) lines: Vec<DiffLine>,
}

#[derive(Clone)]
pub(crate) struct DiffLine {
    pub(crate) kind: String, // "add" | "remove" | "context"
    pub(crate) content: String,
}

impl Workspace {}

pub(crate) fn parse_diff(msg: &BackendMessage) -> Option<DiffRecord> {
    let p = &msg.payload;
    let path = p.get("path")?.as_str()?.to_string();
    let operation = p
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("modify")
        .to_string();
    let mut lines = Vec::new();
    let (mut adds, mut dels) = (0usize, 0usize);
    if let Some(arr) = p.get("diff_lines").and_then(Value::as_array) {
        for l in arr {
            let kind = l
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("context")
                .to_string();
            let content = l
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match kind.as_str() {
                "add" => adds += 1,
                "remove" => dels += 1,
                _ => {}
            }
            lines.push(DiffLine { kind, content });
        }
    }
    Some(DiffRecord {
        path,
        operation,
        adds,
        dels,
        lines,
    })
}

pub(crate) fn op_marker(operation: &str) -> char {
    match operation {
        "create" => 'A',
        "delete" => 'D',
        _ => 'M',
    }
}

/// Parse a unified diff (git output) into renderable lines + add/del counts.
pub(crate) fn parse_unified(text: &str) -> (Vec<DiffLine>, usize, usize) {
    let mut lines = Vec::new();
    let (mut adds, mut dels) = (0usize, 0usize);
    for l in text.lines() {
        if l.starts_with("diff ")
            || l.starts_with("index ")
            || l.starts_with("--- ")
            || l.starts_with("+++ ")
            || l.starts_with("new file")
            || l.starts_with("deleted file")
            || l.starts_with("old mode")
            || l.starts_with("new mode")
            || l.starts_with("similarity")
            || l.starts_with("rename ")
        {
            continue;
        }
        if l.starts_with("@@") {
            lines.push(DiffLine {
                kind: "context".into(),
                content: l.to_string(),
            });
        } else if let Some(rest) = l.strip_prefix('+') {
            adds += 1;
            lines.push(DiffLine {
                kind: "add".into(),
                content: rest.to_string(),
            });
        } else if let Some(rest) = l.strip_prefix('-') {
            dels += 1;
            lines.push(DiffLine {
                kind: "remove".into(),
                content: rest.to_string(),
            });
        } else {
            let rest = l.strip_prefix(' ').unwrap_or(l);
            lines.push(DiffLine {
                kind: "context".into(),
                content: rest.to_string(),
            });
        }
    }
    (lines, adds, dels)
}

pub(crate) fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendMessage;
    use serde_json::json;

    fn msg(payload: serde_json::Value) -> BackendMessage {
        BackendMessage {
            source: String::new(),
            kind: "DiffMessage".into(),
            category: "tool_output".into(),
            text: String::new(),
            payload,
        }
    }

    #[test]
    fn parse_diff_counts_adds_and_dels() {
        let m = msg(json!({
            "path": "src/main.rs",
            "operation": "modify",
            "diff_lines": [
                {"type": "context", "content": "fn main() {"},
                {"type": "add", "content": "    let a = 1;"},
                {"type": "add", "content": "    let b = 2;"},
                {"type": "remove", "content": "    todo!();"}
            ]
        }));
        let d = parse_diff(&m).expect("diff parses");
        assert_eq!(d.path, "src/main.rs");
        assert_eq!(d.operation, "modify");
        assert_eq!(d.adds, 2);
        assert_eq!(d.dels, 1);
        assert_eq!(d.lines.len(), 4);
    }

    #[test]
    fn parse_diff_defaults_operation_and_requires_path() {
        let d = parse_diff(&msg(json!({ "path": "a.txt" }))).unwrap();
        assert_eq!(d.operation, "modify");
        assert_eq!(d.adds, 0);
        assert_eq!(d.dels, 0);
        assert!(parse_diff(&msg(json!({ "operation": "create" }))).is_none());
    }

    #[test]
    fn parse_unified_strips_headers_and_signs() {
        let text = "diff --git a/x b/x\nindex 111..222 100644\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n unchanged\n-old line\n+new line\n";
        let (lines, adds, dels) = parse_unified(text);
        assert_eq!(adds, 1);
        assert_eq!(dels, 1);
        assert_eq!(lines.len(), 4); // hunk header + context + remove + add
        assert_eq!(lines[0].kind, "context");
        assert_eq!(lines[1].content, "unchanged");
        assert_eq!(lines[2].kind, "remove");
        assert_eq!(lines[2].content, "old line");
        assert_eq!(lines[3].kind, "add");
        assert_eq!(lines[3].content, "new line");
    }

    #[test]
    fn op_marker_maps_operations() {
        assert_eq!(op_marker("create"), 'A');
        assert_eq!(op_marker("delete"), 'D');
        assert_eq!(op_marker("modify"), 'M');
        assert_eq!(op_marker("anything-else"), 'M');
    }

    #[test]
    fn file_name_takes_basename() {
        assert_eq!(file_name("src/workspace/diff.rs"), "diff.rs");
        assert_eq!(file_name("plain.txt"), "plain.txt");
    }
}
