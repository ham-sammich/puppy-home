//! Top-level app: supervises Code Puppy workspaces and hosts the dockable shell.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, NodePath, Style};

use crate::browser::BrowserManager;
use crate::dock_layout;
use crate::session::{ComposerStyle, Theme, UiPrefs};
use crate::shell::{Shell, ShellAction, Tab};
use crate::supervisor::Supervisor;
use crate::theme::{Accents, TerminalTheme, ThemePalette};
use crate::workspace::WorkspaceId;

pub struct PuppyApp {
    sup: Supervisor,
    dock: Option<DockState<Tab>>,
    status: String,
    /// A folder-picker dialog runs on a worker thread; its result arrives here.
    /// (Running rfd inline would re-enter the egui frame and crash on Windows.)
    folder_pick: Option<Receiver<Option<PathBuf>>>,
    /// Signature of the last-saved session, to persist only when it changes.
    last_session_sig: String,
    /// Current UI theme (persisted in `session.json`).
    theme: Theme,
    /// Library of saved custom themes (persisted in `themes.json`).
    themes: Vec<ThemePalette>,
    /// The editor's working UI palette (the theme being edited / previewed).
    theme_palette: ThemePalette,
    /// The active terminal palette (persisted in `terminal.json`).
    terminal_theme: TerminalTheme,
    /// Whether the live theme-editor window is open.
    theme_editor_open: bool,
    /// Optional browser plugin: discovery + open browser tabs.
    browser: BrowserManager,
    perf: crate::perf::PerfStats,
    /// State for the MCP Manager tab (one instance).
    mcp: crate::views::mcp_manager::McpManagerView,
    /// State for the Skills Manager tab (one instance).
    skills: crate::views::skills_manager::SkillsManagerView,
    /// State for the Agent Manager tab (one instance).
    agents: crate::views::agent_manager::AgentManagerView,
    /// State for the Dashboard tab (view mode, inline inputs, toasts).
    dashboard: crate::views::dashboard::DashboardView,
    /// Resolved brand/status colors for the active theme (recomputed on theme
    /// change, never per frame).
    accents: Accents,
    /// The chat composer skin (persisted user preference, all workspaces).
    composer_style: ComposerStyle,
    /// Disable decorative animation app-wide (persisted; widgets read it via
    /// ctx data each frame).
    reduce_motion: bool,
    /// State for the Den tab (one instance; holds the live relay link).
    pack: crate::views::den::DenView,
    /// Last activity summary broadcast to the pack (skip resends of the same).
    pack_activity_last: String,
    /// Signature of the last den roster broadcast (skip no-op re-sends).
    den_roster_last: String,
    /// When the den roster was last broadcast (rate-limit gate).
    den_roster_at: std::time::Instant,
    /// When the pack activity was last considered (throttle).
    pack_activity_at: std::time::Instant,
    /// Signature of the last `.puppy/pack.json` written (skip no-op rewrites).
    pack_breadcrumb_sig: String,
    /// When the breadcrumb was last written (periodic freshness re-stamp).
    pack_breadcrumb_at: std::time::Instant,
    /// Whether breadcrumbs are on disk (so leaving the pack cleans them up).
    pack_breadcrumb_written: bool,
    /// Last time the session/dock persistence signature was computed (throttle).
    persist_check_at: std::time::Instant,
    /// The "Connect to remote" dialog, when open.
    remote: Option<crate::views::remote_connect::RemoteConnect>,
    /// An in-flight remote SSH connection (established on a worker thread).
    remote_pending: Option<remote::RemotePending>,
}

mod pack_sync;
mod remote;

/// Apply a theme to the egui context (on launch and on change).
fn apply_theme(ctx: &egui::Context, theme: &Theme, library: &[ThemePalette]) {
    ctx.set_visuals(crate::theme::visuals_for(theme, library));
}

/// Single-instance "panel" tabs that live in the right-side dock zone (the
/// managers today; perf HUD / Puppy Pack chat later). Kept together so opening
/// one carves out — or reuses — a sidebar rather than crowding the main area.
fn is_panel_tab(tab: &Tab) -> bool {
    matches!(
        tab,
        Tab::McpManager | Tab::SkillsManager | Tab::AgentManager | Tab::Pack
    )
}

impl PuppyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::configure(&cc.egui_ctx);
        // Make panel splitters (e.g. the editor/chat divider) easy to grab.
        cc.egui_ctx.global_style_mut(|s| {
            s.interaction.resize_grab_radius_side = 10.0;
            s.interaction.resize_grab_radius_corner = 14.0;
        });
        let saved = crate::session::load();
        let theme = saved.theme;
        let dashboard_mode = saved.dashboard_view;
        let composer_style = saved.composer_style;
        let reduce_motion = saved.reduce_motion;
        let themes = crate::theme::load_themes();
        let terminal_theme = crate::theme::load_terminal();
        // Seed the editor buffer with the active theme (or a fresh dark base).
        let theme_palette = match &theme {
            Theme::Custom(name) => themes
                .iter()
                .find(|t| &t.name == name)
                .cloned()
                .unwrap_or_else(ThemePalette::dark),
            _ => ThemePalette::dark(),
        };
        apply_theme(&cc.egui_ctx, &theme, &themes);
        let mut sup = Supervisor::new(crate::waker::egui_waker(&cc.egui_ctx));
        let mut status = "Open a folder to start a Code Puppy workspace.".to_string();
        let mut opened: Vec<PathBuf> = Vec::new();
        let mut opened_ids: Vec<WorkspaceId> = Vec::new();
        let mut path_to_id: HashMap<String, WorkspaceId> = HashMap::new();

        // Restore the previous session: reopen each saved folder + its agent/model.
        for entry in saved.workspaces {
            let path = PathBuf::from(&entry.path);
            if !path.is_dir() {
                continue; // folder moved/deleted since last run
            }
            if let Ok(id) = sup.open(path.clone()) {
                if let Some(ws) = sup.get_mut(id) {
                    ws.set_restore(entry.agent, entry.model, entry.autosave);
                }
                path_to_id.insert(entry.path.clone(), id);
                opened_ids.push(id);
                opened.push(path);
                status.clear();
            }
        }

        // Rebuild the dock from the saved split layout (remapping workspace
        // paths to fresh ids); else fall back to Dashboard + one chat per folder.
        let mut dock = match &saved.layout {
            Some(layout) => layout.filter_map_tabs(|t| dock_layout::saved_to_tab(t, &path_to_id)),
            None => {
                let mut d = DockState::new(vec![Tab::Dashboard]);
                for id in &opened_ids {
                    d.push_to_focused_leaf(Tab::Chat(*id));
                }
                d
            }
        };
        // Safety net for a stale/partial saved layout: guarantee the Dashboard
        // and a chat tab for every workspace that actually reopened.
        dock_layout::ensure_core_tabs(&mut dock, &opened_ids);

        // Dev convenience: auto-open a workspace folder on launch (if not already).
        if let Ok(path) = std::env::var("PUPPY_HOME_OPEN") {
            let path = PathBuf::from(&path);
            if !opened.contains(&path) {
                match sup.open(path.clone()) {
                    Ok(id) => {
                        dock.push_to_focused_leaf(Tab::Chat(id));
                        opened.push(path);
                        status.clear();
                    }
                    Err(e) => status = format!("Couldn't open {}: {e}", path.display()),
                }
            }
        }

        // Seed the signature so we don't immediately rewrite the same session
        // (agent/model fill in once each sidecar is ready, which then persists).
        let last_session_sig = dock_layout::persist_sig(
            &dock_layout::current_session(
                &sup,
                UiPrefs {
                    theme: theme.clone(),
                    dashboard_view: dashboard_mode,
                    composer_style,
                    reduce_motion,
                },
                Some(&dock),
            ),
            Some(&dock),
        );
        let accents = Accents::from_palette(&crate::theme::palette_for(&theme, &themes));

        PuppyApp {
            sup,
            dock: Some(dock),
            status,
            folder_pick: None,
            last_session_sig,
            theme,
            themes,
            theme_palette,
            terminal_theme,
            theme_editor_open: false,
            browser: BrowserManager::discover(),
            perf: crate::perf::PerfStats::default(),
            mcp: crate::views::mcp_manager::McpManagerView::default(),
            pack: crate::views::den::DenView::default(),
            pack_activity_last: String::new(),
            den_roster_last: String::new(),
            den_roster_at: std::time::Instant::now(),
            pack_activity_at: std::time::Instant::now(),
            pack_breadcrumb_sig: String::new(),
            pack_breadcrumb_at: std::time::Instant::now(),
            pack_breadcrumb_written: false,
            persist_check_at: std::time::Instant::now(),
            skills: crate::views::skills_manager::SkillsManagerView::default(),
            agents: crate::views::agent_manager::AgentManagerView::default(),
            dashboard: crate::views::dashboard::DashboardView::with_mode(dashboard_mode),
            accents,
            composer_style,
            reduce_motion,
            remote: None,
            remote_pending: None,
        }
    }

    /// Persist the open-workspace set whenever it changes (open/close/agent/model).
    fn persist_session(&mut self) {
        // Building the session + signature allocates (it serializes the dock
        // structure), so don't do it every frame -- once a second is plenty,
        // and `on_exit` does a final unconditional pass.
        if self.persist_check_at.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.persist_check_at = std::time::Instant::now();
        self.persist_session_now();
    }

    /// The unthrottled write-if-changed pass (used by the throttle + on exit).
    fn persist_session_now(&mut self) {
        let session = dock_layout::current_session(
            &self.sup,
            UiPrefs {
                theme: self.theme.clone(),
                dashboard_view: self.dashboard.mode,
                composer_style: self.composer_style,
                reduce_motion: self.reduce_motion,
            },
            self.dock.as_ref(),
        );
        let sig = dock_layout::persist_sig(&session, self.dock.as_ref());
        if sig != self.last_session_sig {
            self.last_session_sig = sig;
            crate::session::save(&session);
        }
    }

    /// Open (or focus) a single-instance panel tab in the right-side dock zone.
    /// The first panel splits a sidebar off the main area; later panels cluster
    /// into that same node. egui_dock then lets the user drag them in/out and
    /// resize the split freely.
    fn open_panel_tab(&mut self, tab: Tab) {
        let Some(dock) = self.dock.as_mut() else {
            return;
        };
        // Already open? Just focus it (one instance per kind).
        if let Some(path) =
            dock.find_tab_from(|t| std::mem::discriminant(t) == std::mem::discriminant(&tab))
        {
            let _ = dock.set_active_tab(path);
            return;
        }
        // Cluster beside an existing panel, else carve out the sidebar.
        if let Some(p) = dock.find_tab_from(is_panel_tab) {
            dock.set_focused_node_and_surface(NodePath::new(p.surface, p.node));
            dock.push_to_focused_leaf(tab);
        } else {
            dock.main_surface_mut()
                .split_right(NodeIndex::root(), 0.72, vec![tab]);
        }
    }

    fn open_workspace(&mut self, path: PathBuf) {
        match self.sup.open(path) {
            Ok(id) => {
                if let Some(dock) = self.dock.as_mut() {
                    dock.push_to_focused_leaf(Tab::Chat(id));
                }
                self.status.clear();
            }
            Err(e) => self.status = format!("Couldn't open workspace: {e}"),
        }
    }

    /// Spawn the native folder dialog on a worker thread (never inline).
    fn begin_folder_pick(&mut self, ctx: &egui::Context) {
        if self.folder_pick.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = rfd::FileDialog::new().pick_folder();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.folder_pick = Some(rx);
    }

    fn poll_folder_pick(&mut self) {
        let Some(rx) = &self.folder_pick else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.folder_pick = None;
                if let Some(path) = result {
                    self.open_workspace(path);
                }
            }
            Err(TryRecvError::Disconnected) => self.folder_pick = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Apply structural changes queued during rendering (after the dock drew).
    fn apply_actions(&mut self, actions: Vec<ShellAction>) {
        for action in actions {
            match action {
                ShellAction::OpenFolder(path) => self.open_workspace(path),
                ShellAction::Close(id) => {
                    self.sup.close(id);
                    // Also close any browser tabs that belonged to this workspace.
                    let browser_ids = self.browser.tabs_for_workspace(id);
                    for bid in &browser_ids {
                        self.browser.close_tab(*bid);
                    }
                    if let Some(dock) = self.dock.as_mut() {
                        dock.retain_tabs(|t| match t {
                            Tab::Chat(x) => *x != id,
                            Tab::Browser(b) => !browser_ids.contains(b),
                            _ => true,
                        });
                    }
                }
                ShellAction::FocusChat(id) => {
                    if let Some(dock) = self.dock.as_mut()
                        && let Some(path) =
                            dock.find_tab_from(|t| matches!(t, Tab::Chat(x) if *x == id))
                    {
                        let _ = dock.set_active_tab(path);
                    }
                }
                ShellAction::ShowChanges(id) => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.show_changes();
                    }
                    if let Some(dock) = self.dock.as_mut()
                        && let Some(path) =
                            dock.find_tab_from(|t| matches!(t, Tab::Chat(x) if *x == id))
                    {
                        let _ = dock.set_active_tab(path);
                    }
                }
                ShellAction::Pause(id) => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.pause_turn();
                    }
                }
                ShellAction::Resume(id) => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.resume_turn();
                    }
                }
                ShellAction::Stop(id) => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.stop_turn();
                    }
                }
                ShellAction::Restart(id) => self.sup.restart(id),
                ShellAction::Steer { id, text, queue } => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.steer_text(&text, queue);
                    }
                }
                ShellAction::SendPrompt { id, text } => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.send_prompt_text(&text);
                    }
                }
                ShellAction::SetModel { id, model } => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.set_model_live(&model);
                    }
                }
            }
        }
    }
}

/// The host window's native id, used to embed the browser plugin's window:
/// the HWND on Windows, the NSWindow number on macOS. `None` on Linux (the
/// browser stays a separate window there).
#[cfg(windows)]
fn window_hwnd(frame: &eframe::Frame) -> Option<i64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as i64),
        _ => None,
    }
}

/// macOS: derive the host NSWindow's global window number from the eframe
/// window's NSView (`[[nsView window] windowNumber]`). The browser overlay
/// orders itself just above this number.
#[cfg(target_os = "macos")]
fn window_hwnd(frame: &eframe::Frame) -> Option<i64> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::AppKit(h) => {
            let ns_view = h.ns_view.as_ptr() as *mut Object;
            if ns_view.is_null() {
                return None;
            }
            unsafe {
                let ns_window: *mut Object = msg_send![ns_view, window];
                if ns_window.is_null() {
                    return None;
                }
                let number: isize = msg_send![ns_window, windowNumber];
                Some(number as i64)
            }
        }
        _ => None,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn window_hwnd(_frame: &eframe::Frame) -> Option<i64> {
    None
}

impl eframe::App for PuppyApp {
    fn on_exit(&mut self) {
        // Final layout/session write (the per-frame path is throttled).
        self.persist_session_now();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.perf.on_frame(frame);
        self.sup.drain();
        self.poll_folder_pick();
        self.poll_remote();
        // Keep the den mirror fresh even while its tab is closed (the panel
        // also polls on render; draining twice is harmless), and keep our
        // presence honest (focus + input recency) regardless of visible tab.
        self.pack.poll();
        self.pack.tick_presence(ui.ctx());
        self.broadcast_pack_activity();
        self.broadcast_den_roster();
        self.sync_pack_breadcrumb();

        // Hand the resolved terminal palette to the embedded terminal renderer
        // via the per-context data store (avoids threading it through the dock).
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                crate::theme::terminal_colors_id(),
                self.terminal_theme.resolve(),
            )
        });

        let mut actions: Vec<ShellAction> = Vec::new();
        let mut open_clicked = false;
        let mut open_remote = false;
        let mut open_browser = false;
        let mut open_mcp = false;
        let mut open_skills = false;
        let mut open_agents = false;
        let mut open_pack = false;
        let mut pick_theme: Option<Theme> = None;
        let mut open_editor = false;
        let theme = self.theme.clone();
        let browser_available = self.browser.is_available();

        // Copy the bits the menu needs so its closure doesn't borrow `self`.
        let mut perf_visible = self.perf.visible;
        let mut reduce_motion = self.reduce_motion;
        let picking = self.folder_pick.is_some();
        let ws_count = self.sup.len();
        let waiting = self.sup.waiting_count();
        let status = self.status.clone();
        let theme_names: Vec<String> = self.themes.iter().map(|t| t.name.clone()).collect();

        egui::Panel::top("app-menu").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("puppy-home");
                ui.separator();
                if ui
                    .add_enabled(!picking, egui::Button::new("📁 Open Folder…"))
                    .clicked()
                {
                    open_clicked = true;
                }
                if picking {
                    ui.label(egui::RichText::new("choosing folder…").weak());
                }
                if ui
                    .button("\u{1f517} Connect Remote…")
                    .on_hover_text("Run a Code Puppy on another host over SSH")
                    .clicked()
                {
                    open_remote = true;
                }
                let browser_tip = if browser_available {
                    "Open a browser tab"
                } else {
                    "Browser plugin not installed — opens an install guide"
                };
                if ui.button("Browser").on_hover_text(browser_tip).clicked() {
                    open_browser = true;
                }
                if ui
                    .button("MCP")
                    .on_hover_text("Manage Code Puppy's MCP servers")
                    .clicked()
                {
                    open_mcp = true;
                }
                if ui
                    .button("Skills")
                    .on_hover_text("Manage Code Puppy's skills (SKILL.md)")
                    .clicked()
                {
                    open_skills = true;
                }
                if ui
                    .button("Agents")
                    .on_hover_text("Manage Code Puppy's agents (visual builder)")
                    .clicked()
                {
                    open_agents = true;
                }
                if ui
                    .button(crate::pack::DEN_LABEL)
                    .on_hover_text("Puppy Den — join a room to work a project together")
                    .clicked()
                {
                    open_pack = true;
                }
                ui.label(egui::RichText::new(format!("{ws_count} workspace(s)")).weak());
                if waiting > 0 {
                    ui.separator();
                    ui.colored_label(
                        egui::Color32::from_rgb(215, 156, 220),
                        format!("⚠ {waiting} waiting for input"),
                    );
                }
                if !status.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new(&status).weak());
                }
                ui.separator();
                ui.toggle_value(&mut perf_visible, "perf")
                    .on_hover_text("Performance HUD: frame cost, repaint rate, memory");
                ui.toggle_value(&mut reduce_motion, "\u{1f9d8} calm")
                    .on_hover_text(
                        "Reduce motion: disable decorative animation app-wide \
                         (pulses, ring spins, the empty-state bob)",
                    );
                ui.menu_button(format!("Theme: {}", theme.label()), |ui| {
                    if ui.selectable_label(theme == Theme::Dark, "Dark").clicked() {
                        pick_theme = Some(Theme::Dark);
                        ui.close();
                    }
                    if ui
                        .selectable_label(theme == Theme::Light, "Light")
                        .clicked()
                    {
                        pick_theme = Some(Theme::Light);
                        ui.close();
                    }
                    if !theme_names.is_empty() {
                        ui.separator();
                        ui.label(egui::RichText::new("Custom").weak().small());
                        for name in &theme_names {
                            let sel = matches!(&theme, Theme::Custom(n) if n == name);
                            if ui.selectable_label(sel, name).clicked() {
                                pick_theme = Some(Theme::Custom(name.clone()));
                                ui.close();
                            }
                        }
                    }
                    ui.separator();
                    if ui.button("Edit themes…").clicked() {
                        open_editor = true;
                        ui.close();
                    }
                });
            });
        });

        if open_clicked {
            self.begin_folder_pick(ui.ctx());
        }
        if open_remote && self.remote.is_none() {
            self.remote = Some(crate::views::remote_connect::RemoteConnect::new());
        }
        // Render the remote-connect dialog (if open) and act on its outcome.
        let remote_outcome = self
            .remote
            .as_mut()
            .map(|st| crate::views::remote_connect::render(ui.ctx(), st));
        if let Some(outcome) = remote_outcome {
            if let Some((target, path)) = outcome.connect {
                if let Some(st) = self.remote.as_mut() {
                    st.connecting = true;
                    st.error = None;
                }
                self.begin_remote_connect(target, path, ui.ctx());
            } else if outcome.cancel {
                let connecting = self.remote.as_ref().is_some_and(|s| s.connecting);
                if !connecting {
                    self.remote = None;
                }
            }
        }
        if open_browser {
            let id = self.browser.open_tab(None, None);
            if let Some(dock) = self.dock.as_mut() {
                dock.push_to_focused_leaf(Tab::Browser(id));
            }
        }
        if open_mcp {
            self.open_panel_tab(Tab::McpManager);
        }
        if open_skills {
            self.open_panel_tab(Tab::SkillsManager);
        }
        if open_agents {
            self.open_panel_tab(Tab::AgentManager);
        }
        if open_pack {
            self.open_panel_tab(Tab::Pack);
        }
        if let Some(t) = pick_theme {
            self.theme = t;
            // Sync the editor buffer to the freshly-picked custom theme.
            if let Theme::Custom(name) = &self.theme
                && let Some(p) = self.themes.iter().find(|t| &t.name == name)
            {
                self.theme_palette = p.clone();
            }
            apply_theme(ui.ctx(), &self.theme, &self.themes);
            self.accents =
                Accents::from_palette(&crate::theme::palette_for(&self.theme, &self.themes));
        }
        if open_editor {
            self.theme_editor_open = true;
        }
        if self.theme_editor_open {
            let outcome = crate::theme::editor_window(
                ui.ctx(),
                &mut self.theme_editor_open,
                &mut self.theme_palette,
                &mut self.themes,
                &mut self.terminal_theme,
            );
            if let Some(name) = outcome.select {
                self.theme = Theme::Custom(name);
            }
            if outcome.changed {
                // Live preview: apply the working palette directly (it may not be
                // saved to the library yet).
                ui.ctx().set_visuals(self.theme_palette.to_visuals());
                self.accents = Accents::from_palette(&self.theme_palette);
                if !matches!(self.theme, Theme::Custom(_)) {
                    self.theme = Theme::Custom(self.theme_palette.name.clone());
                }
            }
        }

        // Embedding lifecycle: tell the browser manager our window handle, then
        // bracket the dock draw so it can place/hide plugin windows per-frame.
        let parent_hwnd = window_hwnd(frame);
        self.browser.set_parent_hwnd(parent_hwnd);
        self.browser.begin_frame();

        self.reduce_motion = reduce_motion;
        // Publish the motion preference so every widget can gate its loops.
        crate::views::widgets::set_reduce_motion(ui.ctx(), self.reduce_motion);

        let mut dock = self.dock.take().expect("dock present");
        {
            let mut shell = Shell {
                sup: &mut self.sup,
                browser: &mut self.browser,
                mcp: &mut self.mcp,
                skills: &mut self.skills,
                agents: &mut self.agents,
                pack: &mut self.pack,
                dashboard: &mut self.dashboard,
                accents: &self.accents,
                composer_style: &mut self.composer_style,
                actions: &mut actions,
            };
            DockArea::new(&mut dock)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside(ui, &mut shell);
        }
        self.dock = Some(dock);
        self.perf.visible = perf_visible;
        self.perf.render(ui.ctx());
        self.browser.end_frame();

        // An embedded webview steals OS keyboard focus when clicked; if the user
        // then clicks back onto the egui surface, reclaim focus to the host so
        // the chat box and other fields receive keystrokes again.
        if let Some(parent) = parent_hwnd
            && ui.ctx().input(|i| i.pointer.any_pressed())
        {
            self.browser.reclaim_host_focus(parent);
        }

        self.apply_actions(actions);
        self.persist_session();

        // Keep elapsed timers ticking while any instance is busy.
        if self.sup.any_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
