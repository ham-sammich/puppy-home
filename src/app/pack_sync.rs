//! Den <-> app glue for [`PuppyApp`]: broadcast what this member's puppies are
//! doing (legacy activity strings + the den's typed roster cards), and drop
//! the `.puppy/pack.json` breadcrumb each sidecar reads to inject
//! "[pack context] ..." into prompts (Tier 2). The panel lives in
//! `views::pack_panel`; the wire client + [`DenState`](crate::pack::DenState)
//! mirror in `crate::pack`.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use puppy_relay::protocol::RoomAgentInfo;

use super::PuppyApp;
use crate::workspace::InstanceStatus;

/// Minimum spacing between den roster broadcasts. The status poll ticks every
/// ~1.2s per workspace; the relay must never see that raw cadence.
const DEN_ROSTER_EVERY: Duration = Duration::from_millis(2500);

/// The agent-side coordination CLI (claim/release/claims/post/status), shipped
/// into each workspace's `.puppy/` so agents can run it with plain python.
const PACK_HELPER: &str = include_str!("../../sidecar/pack_helper.py");

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

    /// Broadcast this member's den roster: one compact [`RoomAgentInfo`] per
    /// open workspace, built from state Tasks 0.2/0.3 already maintain. Rate-
    /// limited to [`DEN_ROSTER_EVERY`] and skipped when nothing changed (an
    /// idle den stays silent; tps jitter naturally refreshes busy ones).
    pub(super) fn broadcast_den_roster(&mut self) {
        if !self.pack.connected() || self.den_roster_at.elapsed() < DEN_ROSTER_EVERY {
            return;
        }
        self.den_roster_at = Instant::now();
        let agents: Vec<RoomAgentInfo> = self
            .sup
            .iter()
            .map(|w| {
                let (added, removed) = w.diff_totals();
                let busy = !matches!(w.status, InstanceStatus::Idle | InstanceStatus::Dead);
                RoomAgentInfo {
                    puppy: w.puppy_name.clone(),
                    agent: w.agent.clone(),
                    model: w.model.clone(),
                    state: w.status.label().to_string(),
                    verb: w.current_tool.clone().unwrap_or_default(),
                    file: w.last_file().unwrap_or_default().to_string(),
                    dir: w.name.clone(),
                    // A stale last-rate on an idle agent would lie to the den.
                    tps: if busy { w.token_rate as f32 } else { 0.0 },
                    added,
                    removed,
                }
            })
            .collect();
        let sig = format!("{agents:?}");
        if sig == self.den_roster_last {
            return;
        }
        self.pack.send_roster(agents);
        self.den_roster_last = sig;
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
                for root in &roots {
                    let dir = root.join(".puppy");
                    let _ = std::fs::create_dir_all(&dir);
                    let helper = dir.join("pack_helper.py");
                    let _ = std::fs::write(&helper, PACK_HELPER);
                    // Per-root copy so the breadcrumb can point at ITS helper.
                    let mut per_root = obj.clone();
                    if let Some(map) = per_root.as_object_mut() {
                        map.insert("helper".into(), serde_json::json!(helper.to_string_lossy()));
                    }
                    let text = serde_json::to_string_pretty(&per_root).unwrap_or_default();
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
                            let dir = w.root.join(".puppy");
                            let _ = std::fs::remove_file(dir.join("pack.json"));
                            let _ = std::fs::remove_file(dir.join("pack_helper.py"));
                        }
                    }
                    self.pack_breadcrumb_written = false;
                    self.pack_breadcrumb_sig.clear();
                }
            }
        }
    }
}
