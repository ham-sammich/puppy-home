//! "Connect to a remote folder" dialog.
//!
//! Lets the user pick a host discovered from their `~/.ssh/config` (or type a
//! `[user@]host[:port]` target by hand), choose a remote folder -- typed or via
//! the built-in folder browser (SSH `ls`, starting at the login home) -- and
//! open a workspace whose Code Puppy sidecar runs on that host over SSH. The
//! actual connection happens off-thread in the app; this is just the form.

use crate::backend::ssh::{self, SshTarget};

/// A remote directory listing in flight or done: `(resolved_abs_path, entries)`.
/// (pub(crate): the GPUI connect dialog polls the same result type.)
pub(crate) type ListResult = Result<(String, Vec<(String, bool)>), String>;

/// Run one remote `ls` and parse it — the blocking body shared by both
/// shells' off-thread listing spawns (sync note: extracted in Phase E).
/// `dir` of `None` lists the login home.
pub(crate) fn list_remote_blocking(target: &SshTarget, dir: Option<&str>) -> ListResult {
    match target.list_dir_command(dir).output() {
        Ok(o) if o.status.success() => ssh::parse_listing(&String::from_utf8_lossy(&o.stdout))
            .ok_or_else(|| "Couldn't read that directory.".to_string()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let err = err.trim();
            Err(if err.is_empty() {
                "Listing failed.".to_string()
            } else {
                err.to_string()
            })
        }
        Err(e) => Err(format!("ssh failed: {e}")),
    }
}

/// Join a remote dir + child name without doubling the separator.
pub(crate) fn join_remote(cwd: &str, name: &str) -> String {
    if cwd.ends_with('/') {
        format!("{cwd}{name}")
    } else {
        format!("{cwd}/{name}")
    }
}

/// Parent of an absolute remote path (`None` at the root).
pub(crate) fn parent_remote(cwd: &str) -> Option<String> {
    if cwd == "/" {
        return None;
    }
    match cwd.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => Some("/".to_string()),
        Some((parent, _)) => Some(parent.to_string()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_remote_never_doubles_the_separator() {
        assert_eq!(join_remote("/home/alice", "src"), "/home/alice/src");
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("/var/", "log"), "/var/log");
    }

    #[test]
    fn parent_remote_walks_to_root_and_stops() {
        assert_eq!(parent_remote("/a/b/c"), Some("/a/b".to_string()));
        assert_eq!(parent_remote("/a"), Some("/".to_string()));
        assert_eq!(parent_remote("/a/"), Some("/".to_string()));
        assert_eq!(parent_remote("/"), None);
    }
}
