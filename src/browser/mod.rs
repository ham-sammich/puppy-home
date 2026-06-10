//! The in-app browser, delivered as an optional plugin (Architecture "C").
//!
//! Increment 2: a dockable `Browser` tab that **launches and supervises** the
//! standalone `puppy-browser` process and drives it (navigate/back/forward/
//! reload) over stdin IPC. The webview currently appears in its own OS window;
//! docking it into this panel's rect is the next increment.
//!
//! When the plugin isn't installed, the tab shows an **Install** panel that can
//! install from a local build, open the plugins folder, or rescan.

mod embed;
mod host;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use eframe::egui;

use crate::plugin::{InstalledPlugin, PluginRegistry};
use crate::workspace::WorkspaceId;
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
    /// Whether the plugin window has been reparented into our window yet.
    embedded: bool,
    /// The workspace this browser tab belongs to (if opened from one).
    workspace: Option<WorkspaceId>,
}

/// Owns plugin discovery + the open browser tabs.
pub struct BrowserManager {
    registry: PluginRegistry,
    tabs: BTreeMap<BrowserId, BrowserTab>,
    next_id: u64,
    /// Last install error (shown on the Install panel).
    install_error: Option<String>,
    /// The host window handle to embed plugin windows into (set each frame).
    parent_hwnd: Option<i64>,
    /// Browser tabs whose window was placed (shown) this frame.
    placed: HashSet<BrowserId>,
}

impl BrowserManager {
    /// Build the manager, discovering installed plugins up front.
    pub fn discover() -> Self {
        BrowserManager {
            registry: PluginRegistry::discover(),
            tabs: BTreeMap::new(),
            next_id: 1,
            install_error: None,
            parent_hwnd: None,
            placed: HashSet::new(),
        }
    }

    /// Record the host window handle (call once per frame, before the dock
    /// draws) so embedded plugin windows can be reparented into it.
    pub fn set_parent_hwnd(&mut self, hwnd: Option<i64>) {
        self.parent_hwnd = hwnd;
    }

    /// Start a frame: forget which tabs were placed last frame.
    pub fn begin_frame(&mut self) {
        self.placed.clear();
    }

    /// End a frame: hide any browser window whose tab wasn't drawn (inactive or
    /// closed), so it doesn't float over other views.
    pub fn end_frame(&mut self) {
        for (id, tab) in &self.tabs {
            if self.placed.contains(id) {
                continue;
            }
            if let Some(h) = tab.host.as_ref().and_then(|h| h.child_hwnd()) {
                embed::hide(h);
            }
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

    /// Open a browser tab. When a `url` is given and the plugin is available,
    /// the process launches immediately (e.g. opening a workspace's dev server);
    /// otherwise it shows a Launch button.
    pub fn open_tab(&mut self, workspace: Option<WorkspaceId>, url: Option<String>) -> BrowserId {
        let id = BrowserId(self.next_id);
        self.next_id += 1;
        let mut tab = BrowserTab {
            workspace,
            ..Default::default()
        };
        if let Some(u) = url {
            let u = normalize_url(u.trim());
            let exe = self.registry.get(BROWSER_PLUGIN_ID).map(|p| p.exe_path());
            if self.is_available() {
                launch(&mut tab, exe.as_deref(), &u);
            }
            tab.url = u;
        }
        self.tabs.insert(id, tab);
        id
    }

    /// The browser tabs belonging to a given workspace (for cleanup on close).
    pub fn tabs_for_workspace(&self, ws: WorkspaceId) -> Vec<BrowserId> {
        self.tabs
            .iter()
            .filter(|(_, t)| t.workspace == Some(ws))
            .map(|(id, _)| *id)
            .collect()
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

    /// A short title for the tab strip — the URL host if we have one.
    pub fn tab_title(&self, id: BrowserId) -> String {
        match self.tabs.get(&id).map(|t| t.url.as_str()) {
            Some(u) if !u.is_empty() => format!("Web: {}", host_port(u)),
            _ => "Browser".to_string(),
        }
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

    /// The live browser toolbar; launches/supervises the plugin process and
    /// (on Windows) embeds its window into the viewport below the toolbar.
    fn render_browser(&mut self, ui: &mut egui::Ui, id: BrowserId) {
        let exe = self
            .registry
            .get(BROWSER_PLUGIN_ID)
            .map(|p| p.exe_path());
        let parent = self.parent_hwnd;
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
                if ui.button("Launch browser").clicked() {
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

        // The viewport region below the toolbar: the plugin window is placed here.
        let rect = ui.available_rect_before_wrap();
        let child = tab.host.as_ref().and_then(|h| h.child_hwnd());
        match (embed::SUPPORTED, parent, child) {
            (true, Some(parent), Some(child)) => {
                if !tab.embedded {
                    embed::reparent(parent, child);
                    tab.embedded = true;
                }
                let ppp = ui.ctx().pixels_per_point();
                let x = (rect.min.x * ppp).round() as i32;
                let y = (rect.min.y * ppp).round() as i32;
                let w = (rect.width() * ppp).round() as i32;
                let h = (rect.height() * ppp).round() as i32;
                if w > 0 && h > 0 {
                    embed::place(child, x, y, w, h);
                    self.placed.insert(id);
                }
            }
            (true, _, None) => viewport_note(ui, rect, "starting browser\u{2026}"),
            _ => viewport_note(
                ui,
                rect,
                "Browser running in a separate window.\n(Embedding is Windows-only for now.)",
            ),
        }
    }
}

/// Centered hint painted in the empty viewport region.
fn viewport_note(ui: &egui::Ui, rect: egui::Rect, text: &str) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(14.0),
        ui.visuals().weak_text_color(),
    );
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

/// The `host:port` of a URL, for compact tab titles/chips.
fn host_port(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    after.split(['/', '?', '#']).next().unwrap_or(after).to_string()
}

/// Scan text (e.g. terminal output) for local dev-server URLs to offer opening.
pub fn detect_dev_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for scheme in ["http://", "https://"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(scheme) {
            let abs = from + rel;
            let rest = &text[abs..];
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '<' | '>'))
                .unwrap_or(rest.len());
            let url = rest[..end]
                .trim_end_matches(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'))
                .to_string();
            if is_local_url(&url) && !out.contains(&url) {
                out.push(url);
            }
            from = abs + scheme.len();
        }
    }
    out
}

/// Whether a URL points at the local machine (a dev server worth offering).
fn is_local_url(url: &str) -> bool {
    let hp = host_port(url);
    let host = hp.rsplit_once(':').map(|(h, _)| h).unwrap_or(&hp);
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_local_dev_urls() {
        let log = "  VITE v5  ready\n  Local:   http://localhost:5173/\n  Network: http://192.168.1.5:5173/\n";
        let urls = detect_dev_urls(log);
        assert_eq!(urls, vec!["http://localhost:5173/"]);
    }

    #[test]
    fn detects_multiple_and_dedups() {
        let t = "run on http://127.0.0.1:3000 and again http://127.0.0.1:3000 also 0.0.0.0:8080";
        let urls = detect_dev_urls(t);
        assert_eq!(urls, vec!["http://127.0.0.1:3000"]);
    }

    #[test]
    fn ignores_non_local_urls() {
        assert!(detect_dev_urls("see https://example.com/docs").is_empty());
    }

    #[test]
    fn host_port_extracts() {
        assert_eq!(host_port("http://localhost:5173/app"), "localhost:5173");
        assert_eq!(host_port("https://127.0.0.1:8000"), "127.0.0.1:8000");
    }

    #[test]
    fn normalize_url_adds_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://x.com"), "https://x.com");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("http://x"), "http://x");
    }
}
