//! Pack <-> app glue for [`PuppyApp`]: broadcast what this member's puppies are
//! doing, and drop the `.puppy/pack.json` breadcrumb each sidecar reads to
//! inject "[pack context] ..." into prompts (Tier 2 of Puppy Pack). The Pack
//! panel itself lives in `views::pack_panel`; the wire client in `crate::pack`.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::PuppyApp;

impl PuppyApp {
    /// Tell the pack what this member's puppies are doing -- a compact summary
    /// of every workspace's state, sent only when it changes (checked at most
    /// every couple of seconds). Teammates see it in their member list.
    pub(super) fn broadcast_pack_activity(&mut self) {
        if !self.pack.connected() || self.pack_activity_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.pack_activity_at = Instant::now();
        let parts: Vec<String> = self
            .sup
            .iter()
            .map(|w| {
                let state = w.status.label();
                match &w.current_tool {
                    Some(tool) => format!("{}: {state} ({tool})", w.name),
                    None => format!("{}: {state}", w.name),
                }
            })
            .collect();
        let detail = if parts.is_empty() {
            "no workspaces open".to_string()
        } else {
            parts.join(" · ")
        };
        if detail != self.pack_activity_last {
            self.pack.send_activity("status", &detail);
            self.pack_activity_last = detail;
        }
    }

    /// Tier-2 "work together": drop the pack state at `.puppy/pack.json` in
    /// every LOCAL workspace so each sidecar can inject "[pack context] ..."
    /// into prompts (same breadcrumb mechanism as the browser's CDP note).
    /// Written only when the content changes, plus a periodic re-stamp so the
    /// sidecar's staleness check keeps trusting it; removed on leave.
    pub(super) fn sync_pack_breadcrumb(&mut self) {
        match self.pack.breadcrumb() {
            Some(body) => {
                let roots: Vec<std::path::PathBuf> = self
                    .sup
                    .iter()
                    .filter(|w| !w.is_remote())
                    .map(|w| w.root.clone())
                    .collect();
                let sig = format!("{body}|{roots:?}");
                let restamp = self.pack_breadcrumb_at.elapsed() > Duration::from_secs(300);
                if sig == self.pack_breadcrumb_sig && !restamp {
                    return;
                }
                let mut obj = body;
                if let Some(map) = obj.as_object_mut() {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    map.insert("updated".into(), serde_json::json!(now));
                }
                let text = serde_json::to_string_pretty(&obj).unwrap_or_default();
                for root in &roots {
                    let dir = root.join(".puppy");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("pack.json"), &text);
                }
                self.pack_breadcrumb_sig = sig;
                self.pack_breadcrumb_at = Instant::now();
                self.pack_breadcrumb_written = true;
            }
            None => {
                if self.pack_breadcrumb_written {
                    for w in self.sup.iter() {
                        if !w.is_remote() {
                            let _ = std::fs::remove_file(w.root.join(".puppy").join("pack.json"));
                        }
                    }
                    self.pack_breadcrumb_written = false;
                    self.pack_breadcrumb_sig.clear();
                }
            }
        }
    }
}
