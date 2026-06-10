//! The in-app browser, delivered as an optional plugin (Architecture "C").
//!
//! Increment 2: a dockable `Browser` tab that **launches and supervises** the
//! standalone `puppy-browser` process and drives it (navigate/back/forward/
//! reload) over stdin IPC. The webview currently appears in its own OS window;
//! docking it into this panel's rect is the next increment.
//!
//! When the plugin isn't installed, the tab shows an **Install** panel that can
//! install from a local build, open the plugins folder, or rescan.

mod host;

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui;

use crate::plugin::{InstalledPlugin, PluginRegistry};
use host::BrowserHost;

/// The plugin id this manager drives.
const BROWSER_PLUGIN_ID: &str = "browser";

/// Identifies one open browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserId(pub u64);

/// Per-tab browser state: URL bar + the (optional) running process.
#[derive(Default)]
struct BrowserTab {
    url: String,
    host: Option<BrowserHost>,
    launch_error: Option<String>,
}

/// Owns plugin discovery + the open browser tabs.
pub struct BrowserManager {
    registry: PluginRegistry,
    tabs: BTreeMap<BrowserId, BrowserTab>,
    next_id: u64,
    /// Last install error (shown on the Install panel).
    install_error: Option<String>,
}

impl BrowserManager {
    /// Build the manager, discovering installed plugins up front.
    pub fn discover() -> Self {
        BrowserManager {
            registry: PluginRegistry::discover(),
            tabs: BTreeMap::new(),
            next_id: 1,
            install_error: None,
        }
    }

    /// Whether the browser plugin is installed *and* runnable on this host.
    pub fn is_available(&self) -> bool {
        self.registry
            .get(BROWSER_PLUGIN_ID)
            .map(|p| p.is_runnable())
            .unwrap_or(false)
    }

    /// All discovered plugins (for the dashboard list).
    pub fn plugins(&self) -> &[InstalledPlugin] {
        self.registry.all()
    }

    /// Allocate a fresh tab id (and its backing state).
    pub fn new_tab(&mut self) -> BrowserId {
        let id = BrowserId(self.next_id);
        self.next_id += 1;
        self.tabs.insert(id, BrowserTab::default());
        id
    }

    /// Close a tab: ask its browser process to exit gracefully (drop then also
    /// hard-kills as a backstop).
    pub fn close_tab(&mut self, id: BrowserId) {
        if let Some(mut tab) = self.tabs.remove(&id)
            && let Some(h) = tab.host.as_mut()
        {
            h.close();
        }
    }

    /// A short title for the tab strip.
    pub fn tab_title(&self, _id: BrowserId) -> String {
        "Browser".to_string()
    }

    /// Render one browser tab: Install panel or the live browser toolbar.
    pub fn render_tab(&mut self, ui: &mut egui::Ui, id: BrowserId) {
        if self.is_available() {
            self.render_browser(ui, id);
        } else {
            self.render_install(ui);
        }
    }

    /// The "plugin not installed" panel, with a real local-install path.
    fn render_install(&mut self, ui: &mut egui::Ui) {
        let mut do_install = false;
        let mut do_rescan = false;
        let mut do_open_dir = false;

        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.heading("Browser plugin not installed");
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "The in-app browser is an optional plugin so the base app stays small.",
                )
                .weak(),
            );
            ui.add_space(12.0);

            match self.registry.get(BROWSER_PLUGIN_ID) {
                None => {
                    ui.label("Status: not found.");
                }
                Some(p) if !p.manifest.is_compatible() => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "Found v{} but it needs a newer host (requires host \u{2265} {}).",
                            p.manifest.version, p.manifest.min_host_version
                        ),
                    );
                }
                Some(p) => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Manifest found but executable is missing: {}", p.manifest.exe),
                    );
                }
            }

            ui.add_space(12.0);
            if local_build_exe().is_some()
                && ui
                    .button("Install from local build")
                    .on_hover_text("Copy the freshly-built puppy-browser into the plugins folder")
                    .clicked()
            {
                do_install = true;
            }
            ui.horizontal(|ui| {
                if ui.button("Open plugins folder").clicked() {
                    do_open_dir = true;
                }
                if ui.button("Rescan").clicked() {
                    do_rescan = true;
                }
            });
            if let Some(dir) = self.registry.dir() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.code(dir.display().to_string());
                    if ui.small_button("Copy path").clicked() {
                        ui.ctx().copy_text(dir.display().to_string());
                    }
                });
            }
        });

        if do_install {
            match self.install_from_local_build() {
                Ok(()) => self.registry.rescan(),
                Err(e) => self.install_error = Some(e),
            }
        }
        if do_rescan {
            self.registry.rescan();
        }
        if do_open_dir {
            if let Some(dir) = self.registry.dir() {
                let _ = std::fs::create_dir_all(dir);
                open_in_file_manager(dir);
            }
        }
        if let Some(err) = &self.install_error {
            ui.colored_label(ui.visuals().error_fg_color, err);
        }
    }

    /// Copy the locally-built plugin exe + a manifest into the plugins folder.
    fn install_from_local_build(&self) -> Result<(), String> {
        let exe = local_build_exe().ok_or("No local puppy-browser build found next to the app.")?;
        let dir = self
            .registry
            .dir()
            .ok_or("Could not resolve a plugins directory.")?
            .join(BROWSER_PLUGIN_ID);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let exe_name = exe.file_name().ok_or("bad exe path")?;
        std::fs::copy(&exe, dir.join(exe_name)).map_err(|e| e.to_string())?;
        let manifest = format!(
            r#"{{
  "id": "{BROWSER_PLUGIN_ID}",
  "name": "Web Browser",
  "version": "1.0.0",
  "exe": "{}",
  "min_host_version": "0.0.0"
}}
"#,
            exe_name.to_string_lossy()
        );
        std::fs::write(dir.join("plugin.json"), manifest).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The live browser toolbar; launches/supervises the plugin process.
    fn render_browser(&mut self, ui: &mut egui::Ui, id: BrowserId) {
        let exe = self
            .registry
            .get(BROWSER_PLUGIN_ID)
            .map(|p| p.exe_path());
        let tab = self.tabs.entry(id).or_default();

        // Reap a process that exited (e.g. user closed the browser window).
        if tab.host.as_mut().map(|h| !h.is_alive()).unwrap_or(false) {
            tab.host = None;
        }

        ui.horizontal(|ui| {
            let running = tab.host.is_some();
            ui.add_enabled_ui(running, |ui| {
                if ui.button("\u{2039}").on_hover_text("Back").clicked() {
                    if let Some(h) = tab.host.as_mut() {
                        h.back();
                    }
                }
                if ui.button("\u{203a}").on_hover_text("Forward").clicked() {
                    if let Some(h) = tab.host.as_mut() {
                        h.forward();
                    }
                }
                if ui.button("\u{21bb}").on_hover_text("Reload").clicked() {
                    if let Some(h) = tab.host.as_mut() {
                        h.reload();
                    }
                }
            });

            let go = ui
                .add(
                    egui::TextEdit::singleline(&mut tab.url)
                        .hint_text("Enter a URL…")
                        .desired_width(f32::INFINITY),
                )
                .lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if go && !tab.url.trim().is_empty() {
                let url = normalize_url(tab.url.trim());
                tab.url = url.clone();
                match tab.host.as_mut() {
                    Some(h) => h.navigate(&url),
                    None => launch(tab, exe.as_deref(), &url),
                }
            }
        });
        ui.separator();

        if tab.host.is_none() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                if ui.button("Launch browser window").clicked() {
                    let url = if tab.url.trim().is_empty() {
                        "https://example.com".to_string()
                    } else {
                        normalize_url(tab.url.trim())
                    };
                    tab.url = url.clone();
                    launch(tab, exe.as_deref(), &url);
                }
                if let Some(err) = &tab.launch_error {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
            });
            return;
        }

        // Running: the page is in a separate OS window (docking comes next).
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Browser running in a separate window.\nUse the toolbar above to drive it.\n(Docking it into this panel is the next increment.)",
            egui::FontId::proportional(14.0),
            ui.visuals().weak_text_color(),
        );
    }
}

/// Spawn a host for `tab`, recording any error for display.
fn launch(tab: &mut BrowserTab, exe: Option<&std::path::Path>, url: &str) {
    tab.launch_error = None;
    let Some(exe) = exe else {
        tab.launch_error = Some("Plugin executable not found.".into());
        return;
    };
    match BrowserHost::spawn(exe, url) {
        Ok(h) => tab.host = Some(h),
        Err(e) => tab.launch_error = Some(format!("Couldn't launch browser: {e}")),
    }
}

/// Add a scheme if the user typed a bare host (`example.com` -> `https://…`).
fn normalize_url(input: &str) -> String {
    if input.contains("://") || input.starts_with("about:") {
        input.to_string()
    } else {
        format!("https://{input}")
    }
}

/// The locally-built plugin exe sitting next to the app (dev convenience).
fn local_build_exe() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let name = if cfg!(windows) {
        "puppy-browser.exe"
    } else {
        "puppy-browser"
    };
    let candidate = exe_dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Open a folder in the OS file manager (best-effort).
fn open_in_file_manager(dir: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_url_adds_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://x.com"), "https://x.com");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("http://x"), "http://x");
    }
}
