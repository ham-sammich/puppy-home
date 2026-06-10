//! Persist & restore the set of open workspaces across runs.
//!
//! Saved to a per-user config file (`session.json`); on launch we reopen each
//! folder and re-apply its agent/model once its sidecar is ready.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    #[serde(default)]
    pub theme: Theme,
}

/// UI color theme, persisted across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// The other theme (drives the toggle button).
    pub fn toggled(self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }

    /// Lowercase label for the toggle button.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
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
    fn theme_defaults_dark_and_toggles() {
        assert_eq!(Theme::default(), Theme::Dark);
        assert_eq!(Theme::Dark.toggled(), Theme::Light);
        assert_eq!(Theme::Light.toggled(), Theme::Dark);
        assert_eq!(Theme::Dark.label(), "dark");
        assert_eq!(Theme::Light.label(), "light");
    }

    #[test]
    fn theme_serializes_lowercase() {
        let s = Session {
            workspaces: vec![],
            theme: Theme::Light,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"theme\":\"light\""));
        let back: Session = serde_json::from_str(&j).unwrap();
        assert_eq!(back.theme, Theme::Light);
    }
}
