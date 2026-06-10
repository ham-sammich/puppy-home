//! Optional, separately-installed plugins (Architecture "C": each plugin is a
//! standalone executable the app discovers, then launches + supervises like a
//! Code Puppy sidecar).
//!
//! This module is just the *discovery* layer: it scans a `plugins/` directory
//! for `<id>/plugin.json` manifests and reports which are installed + whether
//! they're compatible with this host version. Launching/IPC lives elsewhere
//! (see `browser.rs` for the first consumer).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// This host application's version (from Cargo), used for compat checks.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A plugin's `plugin.json` manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    /// Stable id, also the manifest's folder name (e.g. `"browser"`).
    pub id: String,
    /// Human-friendly name for the UI.
    pub name: String,
    /// Plugin version (semver `x.y.z`).
    #[serde(default = "zero_version")]
    pub version: String,
    /// Executable to launch, relative to the manifest's folder.
    pub exe: String,
    /// Minimum host version this plugin supports (semver `x.y.z`).
    #[serde(default = "zero_version")]
    pub min_host_version: String,
}

fn zero_version() -> String {
    "0.0.0".to_string()
}

impl PluginManifest {
    /// Whether this plugin is compatible with [`HOST_VERSION`].
    pub fn is_compatible(&self) -> bool {
        version_ge(HOST_VERSION, &self.min_host_version)
    }
}

/// A discovered plugin: its manifest + where it lives on disk.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    /// The plugin's folder (manifest + exe live here).
    pub dir: PathBuf,
}

impl InstalledPlugin {
    /// Absolute path to the plugin's executable.
    pub fn exe_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.exe)
    }

    /// Installed *and* the executable actually exists on disk.
    pub fn is_runnable(&self) -> bool {
        self.manifest.is_compatible() && self.exe_path().is_file()
    }
}

/// The set of plugins discovered at startup (re-scannable).
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<InstalledPlugin>,
    /// The directory scanned (shown to the user for manual installs).
    dir: Option<PathBuf>,
}

impl PluginRegistry {
    /// Scan the plugins directory and build a registry.
    pub fn discover() -> Self {
        let dir = plugins_dir();
        let plugins = dir.as_deref().map(scan_dir).unwrap_or_default();
        PluginRegistry { plugins, dir }
    }

    /// Re-scan (e.g. after an install).
    pub fn rescan(&mut self) {
        self.plugins = self.dir.as_deref().map(scan_dir).unwrap_or_default();
    }

    /// The plugins directory (where users drop installs).
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Look up a plugin by id.
    pub fn get(&self, id: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.manifest.id == id)
    }
}

/// Where plugins live: `$PUPPY_PLUGINS_DIR`, else `<exe-dir>/plugins`, else
/// `<config>/puppy-home/plugins`.
pub fn plugins_dir() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("PUPPY_PLUGINS_DIR") {
        return Some(PathBuf::from(env));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        let next = exe_dir.join("plugins");
        if next.is_dir() {
            return Some(next);
        }
    }
    crate::theme::config_path("plugins")
}

/// Scan a directory for `<sub>/plugin.json` manifests.
fn scan_dir(dir: &Path) -> Vec<InstalledPlugin> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let sub = entry.path();
        if !sub.is_dir() {
            continue;
        }
        if let Some(p) = load_manifest(&sub) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// Parse `<dir>/plugin.json` into an [`InstalledPlugin`].
fn load_manifest(dir: &Path) -> Option<InstalledPlugin> {
    let text = std::fs::read_to_string(dir.join("plugin.json")).ok()?;
    let manifest: PluginManifest = serde_json::from_str(&text).ok()?;
    Some(InstalledPlugin {
        manifest,
        dir: dir.to_path_buf(),
    })
}

/// Parse a `x.y.z` version into a tuple (missing/garbage parts become 0).
fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// `a >= b` by semver ordering.
fn version_ge(a: &str, b: &str) -> bool {
    parse_version(a) >= parse_version(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_ge("1.2.3", "1.2.3"));
        assert!(version_ge("1.3.0", "1.2.9"));
        assert!(version_ge("2.0.0", "1.99.99"));
        assert!(!version_ge("1.2.3", "1.2.4"));
        assert!(!version_ge("0.9.0", "1.0.0"));
    }

    #[test]
    fn version_tolerates_garbage() {
        assert_eq!(parse_version("1.2"), (1, 2, 0));
        assert_eq!(parse_version("oops"), (0, 0, 0));
        assert_eq!(parse_version("3.x.1"), (3, 0, 1));
    }

    #[test]
    fn manifest_parses_with_defaults() {
        let json = r#"{"id":"browser","name":"Web Browser","exe":"puppy-browser.exe"}"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "browser");
        assert_eq!(m.version, "0.0.0");
        assert_eq!(m.min_host_version, "0.0.0");
        assert!(m.is_compatible(), "0.0.0 floor is always compatible");
    }

    #[test]
    fn incompatible_when_host_too_old() {
        let m = PluginManifest {
            id: "x".into(),
            name: "X".into(),
            version: "1.0.0".into(),
            exe: "x".into(),
            min_host_version: "999.0.0".into(),
        };
        assert!(!m.is_compatible());
    }

    #[test]
    fn scan_reads_manifests_from_temp_dir() {
        let root = std::env::temp_dir().join("ph_plugin_scan_test");
        let _ = std::fs::remove_dir_all(&root);
        let bdir = root.join("browser");
        std::fs::create_dir_all(&bdir).unwrap();
        std::fs::write(
            bdir.join("plugin.json"),
            r#"{"id":"browser","name":"Web Browser","version":"1.0.0","exe":"run.exe"}"#,
        )
        .unwrap();

        let found = scan_dir(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.id, "browser");
        assert_eq!(found[0].exe_path(), bdir.join("run.exe"));
        // exe doesn't exist, so not runnable yet.
        assert!(!found[0].is_runnable());

        let _ = std::fs::remove_dir_all(&root);
    }
}
