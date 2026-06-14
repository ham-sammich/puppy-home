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
#[cfg(target_os = "macos")]
pub mod embed_mac;
mod host;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use eframe::egui;

use crate::plugin::{InstalledPlugin, PluginRegistry};
use crate::workspace::WorkspaceId;
use host::BrowserHost;

/// The plugin id this manager drives.
const BROWSER_PLUGIN_ID: &str = "browser";

/// A dependency-free CDP client, embedded so we can hand Code Puppy a ready
/// tool instead of it writing one into the user's project.
const CDP_HELPER_PY: &str = include_str!("../../sidecar/cdp_helper.py");

/// Materialize the CDP helper into app-data (never the project) and return its
/// path. Rewritten only when the embedded content changes.
pub fn ensure_cdp_helper() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("puppy-home");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("cdp.py");
    let stale = std::fs::read_to_string(&path)
        .map(|c| c != CDP_HELPER_PY)
        .unwrap_or(true);
    if stale {
        std::fs::write(&path, CDP_HELPER_PY).ok()?;
    }
    Some(path)
}

/// Identifies one open browser tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserId(pub u64);

#[allow(dead_code)] // consumed by the redesign UI branches
/// Install-panel status (frontend-agnostic mirror of the egui status line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    NotFound,
    Incompatible { version: String, needs: String },
    ExeMissing { exe: String },
    Ready,
}

/// How the GPUI shell presents a browser tab's webview window.
#[allow(dead_code)] // driven by the gpui shell; egui always embeds
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmbedMode {
    /// Borderless overlay glued inside the Browser screen (the default).
    #[default]
    Embedded,
    /// Popped out as a real decorated floating window (\u{2197}).
    Floating,
}

#[allow(dead_code)] // consumed by the redesign UI branches
/// Page operations a shell can drive on a running tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavOp {
    Back,
    Forward,
    Reload,
    DevTools,
}

/// Per-tab browser state: URL bar + the (optional) running process.
#[derive(Default)]
struct BrowserTab {
    url: String,
    host: Option<BrowserHost>,
    launch_error: Option<String>,
    /// Whether the plugin window has been reparented into our window yet
    /// (Windows true-child embedding; unused on the macOS overlay path).
    #[cfg_attr(not(windows), allow(dead_code))]
    embedded: bool,
    /// The workspace this browser tab belongs to (if opened from one).
    workspace: Option<WorkspaceId>,
    /// Whether a process was ever launched (distinguishes "not started yet"
    /// from "started and then exited").
    launched: bool,
    /// Whether the floating-window `float` command was sent to this host
    /// (GPUI shell path; reset on every (re)launch). egui embeds instead
    /// and never floats.
    floated: bool,
    /// GPUI shell presentation mode (egui ignores this — it always
    /// embeds). Default: embedded in the Browser screen; \u{2197} pops out.
    gpui_mode: EmbedMode,
    /// Last physical rect (x, y, w, h) the child window was placed at — so we
    /// only call `SetWindowPos` when it actually changes (avoids choppiness).
    placed_rect: Option<(i32, i32, i32, i32)>,
    /// Whether the child window is currently shown (avoids per-frame ShowWindow).
    visible: bool,
    /// CDP remote-debugging port this page listens on (for DevTools / Code Puppy).
    cdp_port: Option<u16>,
    /// Background placement thread for the embedded overlay — tracks window
    /// drags that GPUI's render loop can't see (Windows modal move loop).
    #[cfg(windows)]
    gluer: Option<embed::Gluer>,
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

    /// Start a frame: reap any exited browser processes and forget which tabs
    /// were placed last frame.
    pub fn begin_frame(&mut self) {
        self.placed.clear();
        for tab in self.tabs.values_mut() {
            if tab.host.as_mut().map(|h| !h.is_alive()).unwrap_or(false) {
                // Process gone: reset embedding state so a relaunch re-attaches.
                tab.host = None;
                tab.embedded = false;
                tab.visible = false;
                tab.placed_rect = None;
            }
        }
    }

    /// Whether a tab's process has exited (or the tab is gone) — so its owner
    /// can drop the now-dead tab. A tab that was never launched is *not* closed.
    pub fn is_tab_closed(&self, id: BrowserId) -> bool {
        self.tabs
            .get(&id)
            .is_none_or(|t| t.launched && t.host.is_none())
    }

    /// The (normalized) URL a tab is pointed at, if any.
    pub fn tab_url(&self, id: BrowserId) -> Option<String> {
        self.tabs.get(&id).map(|t| t.url.clone())
    }

    /// `host:port` of every tab's CDP endpoint — so the dev-server chip scanner
    /// can exclude them (they're our own debugging ports, not the user's app).
    pub fn cdp_hostports(&self) -> Vec<String> {
        self.tabs
            .values()
            .filter_map(|t| t.cdp_port.map(|p| format!("127.0.0.1:{p}")))
            .collect()
    }

    /// The CDP (Chrome DevTools Protocol) endpoint for a tab, if it's running —
    /// e.g. `http://127.0.0.1:9377`. Code Puppy can attach to it to inspect the
    /// console, DOM, network, and evaluate JS in the page.
    pub fn tab_cdp_url(&self, id: BrowserId) -> Option<String> {
        self.tabs
            .get(&id)
            .and_then(|t| t.cdp_port)
            .map(|p| format!("http://127.0.0.1:{p}"))
    }

    /// End a frame: hide any browser window whose tab wasn't drawn (inactive or
    /// closed), so it doesn't float over other views.
    pub fn end_frame(&mut self) {
        for (id, tab) in self.tabs.iter_mut() {
            if self.placed.contains(id) || !tab.visible {
                continue;
            }
            #[cfg(windows)]
            if let Some(h) = tab.host.as_ref().and_then(|h| h.child_hwnd()) {
                embed::hide(h);
            }
            #[cfg(target_os = "macos")]
            if let Some(h) = tab.host.as_mut() {
                h.hide();
            }
            tab.visible = false;
            // Force a reposition next time it's shown (layout may have changed).
            tab.placed_rect = None;
        }
    }

    /// Reclaim OS keyboard focus to the host window when the user clicks the
    /// egui surface — otherwise an embedded webview keeps the keyboard and the
    /// chat box (etc.) won't receive typed input.
    pub fn reclaim_host_focus(&self, parent_hwnd: i64) {
        if let Some(child) = self.tabs.values().find_map(|t| {
            if t.visible && t.embedded {
                t.host.as_ref().and_then(|h| h.child_hwnd())
            } else {
                None
            }
        }) {
            embed::focus_host(parent_hwnd, child);
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

    /// A short title for the tab strip — the file name for `file://`, else host.
    pub fn tab_title(&self, id: BrowserId) -> String {
        match self.tabs.get(&id).map(|t| t.url.as_str()) {
            Some(u) if u.starts_with("file://") => {
                let name = u.rsplit('/').next().unwrap_or(u);
                format!("Web: {name}")
            }
            Some(u) if !u.is_empty() => format!("Web: {}", host_port(u)),
            _ => "Browser".to_string(),
        }
    }

    // -- frontend-agnostic surface (the GPUI shell drives these; the egui
    //    render methods below mutate tabs directly) ------------------------

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Why the Install panel is showing (status line parity with egui's).
    pub fn plugin_status(&self) -> PluginStatus {
        match self.registry.get(BROWSER_PLUGIN_ID) {
            None => PluginStatus::NotFound,
            Some(p) if !p.manifest.is_compatible() => PluginStatus::Incompatible {
                version: p.manifest.version.clone(),
                needs: p.manifest.min_host_version.clone(),
            },
            Some(p) if !p.is_runnable() => PluginStatus::ExeMissing {
                exe: p.manifest.exe.clone(),
            },
            Some(_) => PluginStatus::Ready,
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// The plugins folder (for display / "Open plugins folder").
    pub fn plugins_dir(&self) -> Option<PathBuf> {
        self.registry.dir().map(|d| d.to_path_buf())
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Re-discover installed plugins.
    pub fn rescan(&mut self) {
        self.registry.rescan();
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// "Install from local build": copy + manifest + rescan, recording any
    /// error for the Install panel (egui flow, minus the immediate render).
    pub fn install_local(&mut self) {
        match self.install_from_local_build() {
            Ok(()) => {
                self.install_error = None;
                self.registry.rescan();
            }
            Err(e) => self.install_error = Some(e),
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// The last install failure, if any.
    pub fn install_error(&self) -> Option<&str> {
        self.install_error.as_deref()
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Whether a freshly-built `puppy-browser` sits next to the app.
    pub fn local_build_available() -> bool {
        local_build_exe().is_some()
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Open (creating if needed) the plugins folder in the OS file manager.
    pub fn open_plugins_folder(&self) {
        if let Some(dir) = self.registry.dir() {
            let _ = std::fs::create_dir_all(dir);
            open_in_file_manager(dir);
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Whether a tab's process is currently running (reaps exits first).
    pub fn tab_running(&mut self, id: BrowserId) -> bool {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return false;
        };
        if tab.host.as_mut().map(|h| !h.is_alive()).unwrap_or(false) {
            tab.host = None;
        }
        tab.host.is_some()
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// The last launch failure for a tab, if any.
    pub fn tab_launch_error(&self, id: BrowserId) -> Option<String> {
        self.tabs.get(&id).and_then(|t| t.launch_error.clone())
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Drive a running tab's page (no-ops when the process isn't running).
    pub fn nav(&mut self, id: BrowserId, op: NavOp) {
        if let Some(h) = self.tabs.get_mut(&id).and_then(|t| t.host.as_mut()) {
            match op {
                NavOp::Back => h.back(),
                NavOp::Forward => h.forward(),
                NavOp::Reload => h.reload(),
                NavOp::DevTools => h.devtools(),
            }
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Point a tab at a URL: navigate the running process, or launch one
    /// (the egui URL-bar Enter behavior; input is normalized).
    pub fn navigate_to(&mut self, id: BrowserId, url_text: &str) {
        if url_text.trim().is_empty() {
            return;
        }
        let exe = self.registry.get(BROWSER_PLUGIN_ID).map(|p| p.exe_path());
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        let url = normalize_url(url_text.trim());
        tab.url = url.clone();
        match tab.host.as_mut() {
            Some(h) => h.navigate(&url),
            None => launch(tab, exe.as_deref(), &url),
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// Stop a tab's process but keep the tab (URL stays; relaunchable).
    /// egui has no explicit stop — closing the dock tab kills the process;
    /// the GPUI shell's single surface needs the button.
    pub fn stop_tab(&mut self, id: BrowserId) {
        if let Some(tab) = self.tabs.get_mut(&id) {
            if let Some(h) = tab.host.as_mut() {
                h.close();
            }
            tab.host = None;
            tab.embedded = false;
            tab.visible = false;
            tab.placed_rect = None;
            #[cfg(windows)]
            {
                tab.gluer = None; // stop the placement thread with the process
            }
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    /// The "Launch browser" button: current URL or the example fallback.
    /// GPUI presentation mode of a tab (egui never consults this).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn tab_mode(&self, id: BrowserId) -> EmbedMode {
        self.tabs
            .get(&id)
            .map(|t| t.gpui_mode)
            .unwrap_or(EmbedMode::Embedded)
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn set_tab_mode(&mut self, id: BrowserId, mode: EmbedMode) {
        if let Some(tab) = self.tabs.get_mut(&id) {
            tab.gpui_mode = mode;
            // Mode flips re-drive the window state on the next upkeep.
            tab.floated = false;
            tab.visible = false;
            // Stale placement must not short-circuit the re-glue after a
            // pop-out -> pop-in round trip (G3 retest: pop-in dead).
            tab.placed_rect = None;
        }
    }

    /// Has the plugin reported its window exists?
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn tab_ready(&self, id: BrowserId) -> bool {
        self.tabs
            .get(&id)
            .and_then(|t| t.host.as_ref())
            .is_some_and(|h| h.is_ready())
    }

    /// GPUI embedded mode: glue the plugin overlay to `rect` (global
    /// top-left PHYSICAL px) just above host window `parent`. Sent every
    /// render while the Browser screen is up — same per-frame re-assert
    /// the egui pump does (z-order can reshuffle on any click).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn embed_tab(&mut self, id: BrowserId, rect: (i32, i32, i32, i32), parent: i64) {
        if let Some(tab) = self.tabs.get_mut(&id)
            && let Some(h) = tab.host.as_mut()
        {
            let (x, y, w, h_) = rect;
            if w > 0 && h_ > 0 {
                h.embed(x, y, w, h_, parent);
                tab.visible = true;
                tab.floated = false;
            }
        }
    }

    /// GPUI: order the overlay out (screen switch / minimize). No-op for a
    /// popped-out floating window — that one is MEANT to persist.
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn hide_tab(&mut self, id: BrowserId) {
        if let Some(tab) = self.tabs.get_mut(&id)
            && tab.gpui_mode == EmbedMode::Embedded
            && tab.visible
            && let Some(h) = tab.host.as_mut()
        {
            // Windows: hide via Win32, NOT the IPC `hide` — the host shows
            // the overlay behind tao's back (ShowWindow), so tao believes
            // it's still hidden and its `set_visible(false)` no-ops. That
            // was the G3 "browser takes over the workspace view" bug.
            #[cfg(windows)]
            if let Some(child) = h.child_hwnd() {
                if let Some(g) = &tab.gluer {
                    g.set(None); // pause tracking while hidden
                }
                embed::hide(child);
                tab.visible = false;
                return;
            }
            h.hide();
            tab.visible = false;
        }
    }

    /// GPUI floating mode: show as a real decorated window (sent once per
    /// mode-flip/launch; the plugin call is idempotent anyway). On Windows
    /// a previously reparented child must be released first (untested by
    /// construction — no Windows box here).
    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn float_tab(&mut self, id: BrowserId) {
        if let Some(tab) = self.tabs.get_mut(&id)
            && !tab.floated
            && let Some(h) = tab.host.as_mut()
            && h.is_ready()
        {
            #[cfg(windows)]
            if tab.embedded {
                tab.gluer = None; // stop the placement thread first
                if let Some(child) = h.child_hwnd() {
                    embed::unparent(child);
                }
                tab.embedded = false;
            }
            h.float();
            tab.floated = true;
            tab.visible = true;
        }
    }

    /// GPUI embedded mode on Windows: attach the plugin window as an OWNED
    /// borderless overlay once (NOT `SetParent` — the GPUI window is a
    /// DComp surface that composes over child HWNDs, G3 finding B1), then
    /// hand the canvas rect (host-client px) to the [`embed::Gluer`]
    /// thread, which converts to screen px and repositions on any change —
    /// including host-window moves that GPUI's starved render loop never
    /// sees (modal move loop).
    #[cfg(windows)]
    pub fn embed_tab_win(&mut self, id: BrowserId, parent: i64, rect: (i32, i32, i32, i32)) {
        if let Some(tab) = self.tabs.get_mut(&id)
            && let Some(child) = tab.host.as_ref().and_then(|h| h.child_hwnd())
        {
            if !tab.embedded {
                embed::attach(parent, child);
                tab.gluer = Some(embed::Gluer::spawn(parent, child));
                tab.embedded = true;
                tab.floated = false;
            }
            let (x, y, w, h) = rect;
            if w > 0 && h > 0 {
                // The glue thread does the actual SetWindowPos calls — it
                // also tracks host-window moves that never reach a render.
                if let Some(g) = &tab.gluer {
                    g.set(Some((x, y, w, h)));
                }
                if !tab.visible {
                    embed::show(child);
                    tab.visible = true;
                }
            }
        }
    }

    #[allow(dead_code)] // consumed by the redesign UI branches
    pub fn launch_tab(&mut self, id: BrowserId) {
        let exe = self.registry.get(BROWSER_PLUGIN_ID).map(|p| p.exe_path());
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        let url = if tab.url.trim().is_empty() {
            "https://example.com".to_string()
        } else {
            normalize_url(tab.url.trim())
        };
        tab.url = url.clone();
        launch(tab, exe.as_deref(), &url);
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
                        format!(
                            "Manifest found but executable is missing: {}",
                            p.manifest.exe
                        ),
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
        if do_open_dir && let Some(dir) = self.registry.dir() {
            let _ = std::fs::create_dir_all(dir);
            open_in_file_manager(dir);
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
        let exe = self.registry.get(BROWSER_PLUGIN_ID).map(|p| p.exe_path());
        let parent = self.parent_hwnd;
        let tab = self.tabs.entry(id).or_default();

        // Reap a process that exited (e.g. user closed the browser window).
        if tab.host.as_mut().map(|h| !h.is_alive()).unwrap_or(false) {
            tab.host = None;
        }

        ui.horizontal(|ui| {
            let running = tab.host.is_some();
            ui.add_enabled_ui(running, |ui| {
                if ui.button("\u{2039}").on_hover_text("Back").clicked()
                    && let Some(h) = tab.host.as_mut()
                {
                    h.back();
                }
                if ui.button("\u{203a}").on_hover_text("Forward").clicked()
                    && let Some(h) = tab.host.as_mut()
                {
                    h.forward();
                }
                if ui.button("\u{21bb}").on_hover_text("Reload").clicked()
                    && let Some(h) = tab.host.as_mut()
                {
                    h.reload();
                }
                ui.separator();
                if ui
                    .button("DevTools")
                    .on_hover_text("Open browser DevTools (F12)")
                    .clicked()
                    && let Some(h) = tab.host.as_mut()
                {
                    h.devtools();
                }
                if let Some(port) = tab.cdp_port
                    && ui
                        .button("CDP")
                        .on_hover_text(format!(
                            "Copy DevTools Protocol endpoint http://127.0.0.1:{port} \
                             — paste it to Code Puppy to let it inspect this page"
                        ))
                        .clicked()
                {
                    ui.ctx().copy_text(format!("http://127.0.0.1:{port}"));
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

        // The viewport region below the toolbar is where the plugin window goes.
        let rect = ui.available_rect_before_wrap();

        // Windows: glue the plugin's window over the host as an owned
        // borderless overlay (same strategy as the GPUI shell — SetParent/
        // WS_CHILD is invisible under DComp hosts, G3 finding B1).
        #[cfg(windows)]
        {
            let child = tab.host.as_ref().and_then(|h| h.child_hwnd());
            match (parent, child) {
                (Some(parent), Some(child)) => {
                    if !tab.embedded {
                        embed::attach(parent, child);
                        tab.embedded = true;
                    }
                    let ppp = ui.ctx().pixels_per_point();
                    let x = (rect.min.x * ppp).round() as i32;
                    let y = (rect.min.y * ppp).round() as i32;
                    let w = (rect.width() * ppp).round() as i32;
                    let h = (rect.height() * ppp).round() as i32;
                    if w > 0 && h > 0 {
                        let (sx, sy) = embed::screen_origin(parent, x, y);
                        let r = (sx, sy, w, h);
                        if tab.placed_rect != Some(r) {
                            embed::place(child, sx, sy, w, h);
                            tab.placed_rect = Some(r);
                        }
                        if !tab.visible {
                            embed::show(child);
                            tab.visible = true;
                        }
                        self.placed.insert(id);
                    }
                }
                _ => viewport_note(ui, rect, "starting browser\u{2026}"),
            }
        }

        // macOS: position the borderless plugin window over `rect` (physical
        // screen px) via IPC and z-order it just above the host window. egui's
        // (0,0) is the host content-area top-left, i.e. the viewport inner_rect.
        #[cfg(target_os = "macos")]
        {
            let ready = tab.host.as_ref().map(|h| h.is_ready()).unwrap_or(false);
            let inner = ui.ctx().input(|i| i.viewport().inner_rect);
            match (ready, inner, parent) {
                (true, Some(inner), Some(parent)) => {
                    let ppp = ui.ctx().pixels_per_point();
                    let x = ((inner.min.x + rect.min.x) * ppp).round() as i32;
                    let y = ((inner.min.y + rect.min.y) * ppp).round() as i32;
                    let w = (rect.width() * ppp).round() as i32;
                    let h = (rect.height() * ppp).round() as i32;
                    if w > 0 && h > 0 {
                        // Re-place + re-order every frame: the overlay is a
                        // separate window, so clicking the host (or another
                        // app) can reshuffle z-order; re-asserting keeps it
                        // glued above the host and tracking moves/resizes.
                        if let Some(host) = tab.host.as_mut() {
                            host.embed(x, y, w, h, parent);
                        }
                        tab.placed_rect = Some((x, y, w, h));
                        tab.visible = true;
                        self.placed.insert(id);
                        ui.ctx().request_repaint();
                    }
                }
                (false, _, _) => viewport_note(ui, rect, "starting browser\u{2026}"),
                _ => viewport_note(ui, rect, "positioning browser\u{2026}"),
            }
        }

        // Linux: no in-tab embedding yet; the webview stays a separate window.
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (id, parent);
            viewport_note(
                ui,
                rect,
                "Browser running in a separate window.\n(In-tab embedding isn't available on this platform yet.)",
            );
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
    // Give every page a CDP endpoint so DevTools / Code Puppy can attach.
    let cdp_port = pick_free_port();
    match BrowserHost::spawn(exe, url, cdp_port) {
        Ok(h) => {
            tab.host = Some(h);
            tab.launched = true;
            tab.floated = false; // fresh process: not yet shown anywhere
            tab.cdp_port = cdp_port;
        }
        Err(e) => tab.launch_error = Some(format!("Couldn't launch browser: {e}")),
    }
}

/// Grab an ephemeral free TCP port for the page's CDP remote-debugging endpoint.
fn pick_free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
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

/// Whether a path looks like an HTML file we can preview in the browser.
pub(crate) fn is_html(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    )
}

/// A compact label for a dev-server / file chip: host:port for http(s),
/// or the file name for a `file://` URL.
pub(crate) fn url_chip_label(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("file://") {
        let path = rest.trim_start_matches('/');
        return path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(rest)
            .to_string();
    }
    host_port(url)
}

/// Build a `file://` URL for a local path, so the plugin can open local HTML.
pub fn file_url(path: &std::path::Path) -> String {
    let s = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(' ', "%20");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// The `host:port` of a URL, for compact tab titles/chips.
pub(crate) fn host_port(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    after
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after)
        .to_string()
}

/// Where a candidate URL token ends (first whitespace/bracket/quote).
fn url_token_end(rest: &str) -> usize {
    rest.find(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '<' | '>')
    })
    .unwrap_or(rest.len())
}

/// Scan text (e.g. terminal output / agent chatter) for local dev-server URLs
/// to offer opening. Catches full `http(s)://` URLs AND scheme-less mentions
/// like `localhost:8000/x` (agents often print those bare) — assumed http.
pub fn detect_dev_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // 1) Full http(s):// URLs.
    for scheme in ["http://", "https://"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(scheme) {
            let abs = from + rel;
            let rest = &text[abs..];
            let end = url_token_end(rest);
            let url = rest[..end]
                .trim_end_matches(['.', ',', ';', ':', '!', '?', '/'])
                .to_string();
            if is_local_url(&url) && !out.contains(&url) {
                out.push(url);
            }
            from = abs + scheme.len();
        }
    }
    // 2) Scheme-less local hosts, e.g. "serve it at localhost:8000/haiku.html".
    //    Require a ':port' right after the host, and skip occurrences that are
    //    actually part of a scheme:// URL (already handled above).
    for host in ["localhost", "127.0.0.1", "0.0.0.0"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(host) {
            let abs = from + rel;
            let after = &text[abs..];
            let tail = &after[host.len()..];
            let preceded_by_scheme = text[..abs].ends_with("://");
            // Need host:DIGIT, and not glued to a larger token / scheme URL.
            let is_port = tail
                .strip_prefix(':')
                .and_then(|s| s.chars().next())
                .is_some_and(|c| c.is_ascii_digit());
            if preceded_by_scheme || !is_port {
                from = abs + host.len();
                continue;
            }
            let end = url_token_end(after);
            let token = after[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '/']);
            let url = format!("http://{token}");
            if !out.contains(&url) {
                out.push(url);
            }
            from = abs + host.len();
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
        assert_eq!(urls, vec!["http://localhost:5173"]);
    }

    #[test]
    fn detects_multiple_and_dedups() {
        // The scheme'd 127.0.0.1 dedups; the bare 0.0.0.0:8080 is now also
        // picked up (assumed http) so agents that print bare hosts still work.
        let t = "run on http://127.0.0.1:3000 and again http://127.0.0.1:3000/ also 0.0.0.0:8080";
        let urls = detect_dev_urls(t);
        assert_eq!(
            urls,
            vec!["http://127.0.0.1:3000", "http://0.0.0.0:8080"]
        );
    }

    #[test]
    fn detects_scheme_less_localhost() {
        // The exact shape code_puppy printed: "serve it at localhost:8000/x".
        let t = "I can fire up server.py and serve it at localhost:8000/haiku.html .";
        let urls = detect_dev_urls(t);
        assert_eq!(urls, vec!["http://localhost:8000/haiku.html"]);
    }

    #[test]
    fn ignores_non_local_urls() {
        assert!(detect_dev_urls("see https://example.com/docs").is_empty());
    }

    #[test]
    fn file_url_builds() {
        use std::path::Path;
        assert_eq!(
            file_url(Path::new("D:\\proj\\index.html")),
            "file:///D:/proj/index.html"
        );
        assert_eq!(
            file_url(Path::new("D:\\a b\\x.html")),
            "file:///D:/a%20b/x.html"
        );
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
        // file:// survives so the plugin can preview local HTML.
        assert_eq!(normalize_url("file:///c:/x/i.html"), "file:///c:/x/i.html");
    }

    #[test]
    fn is_html_matches_extensions() {
        use std::path::Path;
        assert!(is_html(Path::new("a/b/index.html")));
        assert!(is_html(Path::new("page.HTM")));
        assert!(!is_html(Path::new("main.rs")));
        assert!(!is_html(Path::new("noext")));
    }

    #[test]
    fn chip_label_uses_filename_for_file_urls() {
        assert_eq!(
            url_chip_label("http://localhost:5173/app"),
            "localhost:5173"
        );
        assert_eq!(
            url_chip_label("file:///c:/dev/site/index.html"),
            "index.html"
        );
    }
}
