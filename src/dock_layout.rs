//! Glue between the runtime dock (`DockState<Tab>`) and the persisted,
//! device-independent layout (`DockState<SavedTab>`).
//!
//! Two jobs: snapshot the live dock into a `Session` for `session.json`
//! (mapping runtime ids to stable workspace paths, dropping Browser tabs), and
//! compute a change signature that captures the dock's *structure* (tab
//! arrangement, active tab, split fractions) while ignoring per-frame node
//! rects — so resizing the OS window never churns the file, but dragging a
//! split or moving a tab does.

use std::collections::HashMap;

use egui_dock::{DockState, Node};

use crate::session::{SavedTab, Session, Theme, WorkspaceEntry, normalize_layout_rects};
use crate::shell::Tab;
use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceId;

/// Snapshot the open workspaces + dock layout as a persistable session.
/// UI prefs not owned by the egui shell (dashboard_view / reduce_motion are
/// written by the GPUI dashboard) are carried over from the saved session so
/// an egui-shell save never clobbers them.
pub fn current_session(sup: &Supervisor, theme: Theme, dock: Option<&DockState<Tab>>) -> Session {
    let carry = crate::session::load();
    Session {
        dashboard_view: carry.dashboard_view,
        reduce_motion: carry.reduce_motion,
        workspaces: sup
            .iter()
            .map(|w| WorkspaceEntry {
                path: w.root.to_string_lossy().into_owned(),
                agent: (!w.agent.is_empty()).then(|| w.agent.clone()),
                model: (!w.model.is_empty()).then(|| w.model.clone()),
                autosave: (!w.autosave.is_empty()).then(|| w.autosave.clone()),
            })
            .collect(),
        theme,
        // Map runtime tabs to stable keys; Browser tabs (and chats whose
        // workspace has closed) drop out, collapsing any emptied nodes. Rects
        // are normalized so the result always serializes.
        layout: dock.map(|d| {
            let mut saved = d.filter_map_tabs(|t| tab_to_saved(t, sup));
            normalize_layout_rects(&mut saved);
            saved
        }),
    }
}

/// Map a runtime [`Tab`] to its persistable [`SavedTab`], or `None` to drop it
/// (Browser tabs, or chats whose workspace is no longer open).
fn tab_to_saved(tab: &Tab, sup: &Supervisor) -> Option<SavedTab> {
    match tab {
        Tab::Dashboard => Some(SavedTab::Dashboard),
        Tab::Chat(id) => sup
            .get(*id)
            .map(|w| SavedTab::Chat(w.root.to_string_lossy().into_owned())),
        Tab::Browser(_) => None,
        Tab::McpManager => Some(SavedTab::McpManager),
        Tab::SkillsManager => Some(SavedTab::SkillsManager),
        Tab::AgentManager => Some(SavedTab::AgentManager),
        Tab::Pack => Some(SavedTab::Pack),
    }
}

/// Map a persisted [`SavedTab`] back to a runtime [`Tab`], remapping a chat's
/// workspace path to the freshly spawned id (or `None` if that folder didn't
/// reopen this run).
pub fn saved_to_tab(saved: &SavedTab, ids: &HashMap<String, WorkspaceId>) -> Option<Tab> {
    match saved {
        SavedTab::Dashboard => Some(Tab::Dashboard),
        SavedTab::Chat(path) => ids.get(path).map(|id| Tab::Chat(*id)),
        SavedTab::McpManager => Some(Tab::McpManager),
        SavedTab::SkillsManager => Some(Tab::SkillsManager),
        SavedTab::AgentManager => Some(Tab::AgentManager),
        SavedTab::Pack => Some(Tab::Pack),
    }
}

/// Guarantee the non-closeable Dashboard and a chat tab per reopened workspace
/// exist, in case a restored layout was stale (folders added/removed since).
pub fn ensure_core_tabs(dock: &mut DockState<Tab>, ids: &[WorkspaceId]) {
    if dock
        .find_tab_from(|t| matches!(t, Tab::Dashboard))
        .is_none()
    {
        dock.push_to_focused_leaf(Tab::Dashboard);
    }
    for id in ids {
        if dock
            .find_tab_from(|t| matches!(t, Tab::Chat(x) if *x == *id))
            .is_none()
        {
            dock.push_to_focused_leaf(Tab::Chat(*id));
        }
    }
}

/// An in-process identity key for a tab in the structural signature; `None`
/// for tabs we don't persist (Browser), so they don't trigger session writes.
fn tab_key(tab: &Tab) -> Option<String> {
    match tab {
        Tab::Dashboard => Some("D".into()),
        Tab::Chat(id) => Some(format!("C{}", id.0)),
        Tab::Browser(_) => None,
        Tab::McpManager => Some("M".into()),
        Tab::SkillsManager => Some("S".into()),
        Tab::AgentManager => Some("A".into()),
        Tab::Pack => Some("P".into()),
    }
}

/// Signature of the dock's structure (arrangement, active tab, split fractions)
/// that ignores per-frame node rects. See the module docs for the rationale.
fn layout_struct_sig(dock: &DockState<Tab>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for (i, (_path, node)) in dock.iter_all_nodes().enumerate() {
        let _ = write!(out, "{i}");
        match node {
            Node::Empty => out.push('E'),
            Node::Leaf(leaf) => {
                let _ = write!(out, "L{}", leaf.active.0);
                for t in &leaf.tabs {
                    if let Some(k) = tab_key(t) {
                        out.push(':');
                        out.push_str(&k);
                    }
                }
            }
            Node::Vertical(s) => {
                let _ = write!(out, "V{:.3}", s.fraction);
            }
            Node::Horizontal(s) => {
                let _ = write!(out, "H{:.3}", s.fraction);
            }
        }
        out.push('|');
    }
    out
}

/// Combined change signature: workspaces + theme (the volatile layout excluded)
/// plus the rect-free structural layout signature.
pub fn persist_sig(session: &Session, dock: Option<&DockState<Tab>>) -> String {
    let head = {
        let mut bare = session.clone();
        bare.layout = None;
        serde_json::to_string(&bare).unwrap_or_default()
    };
    match dock {
        Some(d) => format!("{head}|{}", layout_struct_sig(d)),
        None => head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_to_tab_remaps_known_paths_only() {
        let mut ids = HashMap::new();
        ids.insert("/proj/a".to_string(), WorkspaceId(7));
        // A known path remaps to its fresh id.
        assert_eq!(
            saved_to_tab(&SavedTab::Chat("/proj/a".into()), &ids),
            Some(Tab::Chat(WorkspaceId(7)))
        );
        // An unknown path (folder not reopened) drops out.
        assert_eq!(
            saved_to_tab(&SavedTab::Chat("/proj/gone".into()), &ids),
            None
        );
        // Singleton panels and the dashboard always map.
        assert_eq!(
            saved_to_tab(&SavedTab::Dashboard, &ids),
            Some(Tab::Dashboard)
        );
        assert_eq!(
            saved_to_tab(&SavedTab::McpManager, &ids),
            Some(Tab::McpManager)
        );
    }

    #[test]
    fn tab_key_skips_browser_only() {
        assert_eq!(tab_key(&Tab::Dashboard).as_deref(), Some("D"));
        assert_eq!(tab_key(&Tab::Chat(WorkspaceId(3))).as_deref(), Some("C3"));
        assert_eq!(tab_key(&Tab::McpManager).as_deref(), Some("M"));
        assert!(tab_key(&Tab::Browser(crate::browser::BrowserId(1))).is_none());
    }

    #[test]
    fn struct_sig_changes_with_active_tab_not_rects() {
        let mut dock = DockState::new(vec![Tab::Dashboard, Tab::McpManager]);
        let before = layout_struct_sig(&dock);
        // Switching the active tab is a real structural change.
        if let Some(leaf) = dock
            .main_surface_mut()
            .iter_mut()
            .find_map(Node::get_leaf_mut)
        {
            leaf.active = egui_dock::TabIndex(1);
        }
        assert_ne!(before, layout_struct_sig(&dock));
    }
}
