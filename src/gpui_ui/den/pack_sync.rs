//! Den <-> app glue the egui shell runs in `app/pack_sync.rs`, ported for
//! the GPUI drain loop: the legacy activity-string broadcast (what
//! teammates' member lists show) and the `.puppy/pack.json` Tier-2
//! breadcrumb each sidecar reads to inject "[pack context] ..." into
//! prompts. The roster broadcast (the third pack_sync job) already lives
//! in `den::actions::den_upkeep`.
//!
//! Cadence note: egui runs these per frame; here the drain loop calls
//! `pack_sync_upkeep` every 250-1000ms, comfortably inside the 2s/300s
//! rate gates both behaviors carry.

use std::time::{Duration, Instant};

use crate::gpui_ui::RootView;
use crate::pack::{remove_pack_breadcrumb, write_pack_breadcrumb};

/// Minimum spacing between activity broadcasts (egui `pack_sync` value).
const ACTIVITY_EVERY: Duration = Duration::from_secs(2);
/// Re-stamp the breadcrumb this often even when unchanged, so the
/// sidecar's staleness check keeps trusting it.
const BREADCRUMB_RESTAMP: Duration = Duration::from_secs(300);

impl RootView {
    /// Drain-loop entry: both pack_sync jobs, each self-gated.
    pub(crate) fn pack_sync_upkeep(&mut self) {
        self.broadcast_pack_activity();
        self.sync_pack_breadcrumb();
    }

    /// Tell the pack what this member's puppies are doing -- a compact
    /// summary of every workspace's state, sent only when it changes
    /// (checked at most every couple of seconds).
    fn broadcast_pack_activity(&mut self) {
        if !self.den.as_ref().is_some_and(|d| d.alive)
            || self
                .pack_activity_at
                .is_some_and(|at| at.elapsed() < ACTIVITY_EVERY)
        {
            return;
        }
        self.pack_activity_at = Some(Instant::now());
        let parts: Vec<String> = self
            .supervisor
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
            parts.join(" \u{b7} ")
        };
        if detail != self.pack_activity_last {
            if let Some(den) = &self.den {
                den.client.activity("status", &detail);
            }
            self.pack_activity_last = detail;
        }
    }

    /// Tier-2 "work together": drop the pack state at `.puppy/pack.json`
    /// in every LOCAL workspace; written only on change (plus the periodic
    /// re-stamp), removed once the den connection goes away.
    fn sync_pack_breadcrumb(&mut self) {
        let body = self.den.as_ref().filter(|d| d.alive).map(|d| {
            d.state
                .breadcrumb_body(&d.room, &d.addr, &d.user, &self.puppy_name())
        });
        let roots: Vec<std::path::PathBuf> = self
            .supervisor
            .iter()
            .filter(|w| !w.is_remote())
            .map(|w| w.root.clone())
            .collect();
        match body {
            Some(body) => {
                let sig = format!("{body}|{roots:?}");
                let restamp = self
                    .pack_breadcrumb_at
                    .is_none_or(|at| at.elapsed() > BREADCRUMB_RESTAMP);
                if sig == self.pack_breadcrumb_sig && !restamp {
                    return;
                }
                write_pack_breadcrumb(&roots, &body);
                self.pack_breadcrumb_sig = sig;
                self.pack_breadcrumb_at = Some(Instant::now());
                self.pack_breadcrumb_written = true;
            }
            None => {
                if self.pack_breadcrumb_written {
                    remove_pack_breadcrumb(&roots);
                    self.pack_breadcrumb_written = false;
                    self.pack_breadcrumb_sig.clear();
                }
            }
        }
    }
}
