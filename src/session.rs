//! Persist & restore the set of open workspaces across runs.
//!
//! Saved to a per-user config file (`session.json`); on launch we reopen each
//! folder and re-apply its agent/model once its sidecar is ready.

use std::path::PathBuf;

use eframe::egui::Rect;
use egui_dock::DockState;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    #[serde(default)]
    pub theme: Theme,
    /// The egui_dock split layout, in device-independent terms (workspace
    /// paths, not runtime ids). Restored by remapping paths back to freshly
    /// spawned [`WorkspaceId`](crate::workspace::WorkspaceId)s. Absent on first
    /// run or pre-layout sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<DockState<SavedTab>>,
}

/// A persistable mirror of `shell::Tab` using stable keys instead of runtime
/// ids, so the dock layout survives a restart. Browser tabs are intentionally
/// omitted (the browser plugin doesn't restore tabs across runs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedTab {
    Dashboard,
    /// A workspace chat tab, keyed by the workspace's folder path.
    Chat(String),
    McpManager,
    SkillsManager,
    AgentManager,
}

/// UI color theme, persisted across runs. `Custom(name)` references a saved
/// theme in `themes.json` by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Custom(String),
}

impl Theme {
    /// Human-friendly label for the theme menu.
    pub fn label(&self) -> String {
        match self {
            Theme::Dark => "Dark".into(),
            Theme::Light => "Light".into(),
            Theme::Custom(name) => name.clone(),
        }
    }

    /// Compact persisted token: `dark`, `light`, or `custom:<name>`.
    fn token(&self) -> String {
        match self {
            Theme::Dark => "dark".into(),
            Theme::Light => "light".into(),
            Theme::Custom(name) => format!("custom:{name}"),
        }
    }

    /// Parse a persisted token back into a [`Theme`] (unknown -> Dark).
    fn from_token(s: &str) -> Theme {
        match s {
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            other => match other.strip_prefix("custom:") {
                Some(name) => Theme::Custom(name.to_string()),
                None => Theme::Dark,
            },
        }
    }
}

impl Serialize for Theme {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.token())
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Theme, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Theme::from_token(&s))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Code Puppy autosave session this workspace was tied to (to resume it).
    #[serde(default)]
    pub autosave: Option<String>,
}

/// Reset every node rect to a finite value before serializing. Fresh / not-yet
/// laid-out nodes carry `Rect::NOTHING` (infinity), which JSON renders as
/// `null` and then refuses to deserialize back into `f32`. egui_dock recomputes
/// all rects each frame, so zeroing them on save is lossless.
pub fn normalize_layout_rects(dock: &mut DockState<SavedTab>) {
    for (_path, node) in dock.iter_all_nodes_mut() {
        node.set_rect(Rect::ZERO);
        if let Some(leaf) = node.get_leaf_mut() {
            leaf.viewport = Rect::ZERO;
        }
    }
}

/// Per-OS config file location: `<config-dir>/puppy-home/session.json`.
fn session_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("puppy-home").join("session.json"))
}

/// Load the saved session (empty if missing or unreadable).
pub fn load() -> Session {
    let Some(path) = session_path() else {
        return Session::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Session::default(),
    }
}

/// Write the session to disk (best-effort; errors are ignored).
pub fn save(session: &Session) {
    let Some(path) = session_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(session) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join("ph_session_test");
        let _ = std::fs::remove_dir_all(&dir);
        // Point every OS's config base at the temp dir for this test.
        unsafe {
            std::env::set_var("APPDATA", &dir);
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            std::env::set_var("HOME", &dir);
        }

        let session = Session {
            workspaces: vec![
                WorkspaceEntry {
                    path: "D:/proj/a".into(),
                    agent: Some("code-puppy".into()),
                    model: Some("gpt-5".into()),
                    autosave: Some("auto_session_20260101_000000".into()),
                },
                WorkspaceEntry {
                    path: "D:/proj/b".into(),
                    agent: None,
                    model: None,
                    autosave: None,
                },
            ],
            theme: Theme::Light,
            layout: None,
        };
        save(&session);

        let loaded = load();
        assert_eq!(loaded.workspaces.len(), 2);
        assert_eq!(loaded.workspaces[0].path, "D:/proj/a");
        assert_eq!(loaded.workspaces[0].agent.as_deref(), Some("code-puppy"));
        assert_eq!(loaded.workspaces[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(
            loaded.workspaces[0].autosave.as_deref(),
            Some("auto_session_20260101_000000")
        );
        assert_eq!(loaded.workspaces[1].agent, None);
        assert_eq!(loaded.theme, Theme::Light);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_or_missing_is_default() {
        let s: Session = serde_json::from_str("{}").unwrap();
        assert!(s.workspaces.is_empty());
        assert_eq!(s.theme, Theme::Dark);
    }

    #[test]
    fn theme_defaults_dark_and_labels() {
        assert_eq!(Theme::default(), Theme::Dark);
        assert_eq!(Theme::Dark.label(), "Dark");
        assert_eq!(Theme::Custom("Neon".into()).label(), "Neon");
    }

    #[test]
    fn custom_theme_roundtrips_via_serde() {
        let s = Session {
            workspaces: vec![],
            theme: Theme::Custom("Neon".into()),
            layout: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"theme\":\"custom:Neon\""));
        let back: Session = serde_json::from_str(&j).unwrap();
        assert_eq!(back.theme, Theme::Custom("Neon".into()));
    }

    #[test]
    fn layout_roundtrips_via_serde() {
        let mut dock = DockState::new(vec![SavedTab::Dashboard, SavedTab::McpManager]);
        normalize_layout_rects(&mut dock); // fresh leaves carry inf rects
        let s = Session {
            workspaces: vec![],
            theme: Theme::Dark,
            layout: Some(dock),
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&j).unwrap();
        let tabs: Vec<_> = back
            .layout
            .unwrap()
            .iter_all_tabs()
            .map(|(_, t)| t.clone())
            .collect();
        assert_eq!(tabs, vec![SavedTab::Dashboard, SavedTab::McpManager]);
    }

    #[test]
    fn layout_absent_when_none() {
        let s = Session::default();
        let j = serde_json::to_string(&s).unwrap();
        assert!(!j.contains("layout"));
    }

    #[test]
    fn theme_serializes_lowercase() {
        let s = Session {
            workspaces: vec![],
            theme: Theme::Light,
            layout: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"theme\":\"light\""));
        let back: Session = serde_json::from_str(&j).unwrap();
        assert_eq!(back.theme, Theme::Light);
    }
}
