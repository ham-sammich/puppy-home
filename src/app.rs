//! Top-level app: supervises Code Puppy workspaces and hosts the dockable shell.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui_dock::{DockArea, DockState, Style};

use crate::browser::BrowserManager;
use crate::session::Theme;
use crate::shell::{Shell, ShellAction, Tab};
use crate::supervisor::Supervisor;
use crate::theme::{TerminalTheme, ThemePalette};

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
}

/// Snapshot the open workspaces as a persistable session.
fn current_session(sup: &Supervisor, theme: Theme) -> crate::session::Session {
    crate::session::Session {
        workspaces: sup
            .iter()
            .map(|w| crate::session::WorkspaceEntry {
                path: w.root.to_string_lossy().into_owned(),
                agent: (!w.agent.is_empty()).then(|| w.agent.clone()),
                model: (!w.model.is_empty()).then(|| w.model.clone()),
                autosave: (!w.autosave.is_empty()).then(|| w.autosave.clone()),
            })
            .collect(),
        theme,
    }
}

/// Apply a theme to the egui context (on launch and on change).
fn apply_theme(ctx: &egui::Context, theme: &Theme, library: &[ThemePalette]) {
    ctx.set_visuals(crate::theme::visuals_for(theme, library));
}

fn session_sig(session: &crate::session::Session) -> String {
    serde_json::to_string(session).unwrap_or_default()
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
        let mut sup = Supervisor::new(cc.egui_ctx.clone());
        let mut dock = DockState::new(vec![Tab::Dashboard]);
        let mut status = "Open a folder to start a Code Puppy workspace.".to_string();
        let mut opened: Vec<PathBuf> = Vec::new();

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
                dock.push_to_focused_leaf(Tab::Chat(id));
                opened.push(path);
                status.clear();
            }
        }

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
        let last_session_sig = session_sig(&current_session(&sup, theme.clone()));

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
        }
    }

    /// Persist the open-workspace set whenever it changes (open/close/agent/model).
    fn persist_session(&mut self) {
        let session = current_session(&self.sup, self.theme.clone());
        let sig = session_sig(&session);
        if sig != self.last_session_sig {
            self.last_session_sig = sig;
            crate::session::save(&session);
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
            }
        }
    }
}

/// The host window's native handle (Windows only), used to embed the browser
/// plugin's window. `None` everywhere else (the browser stays a separate window).
#[cfg(windows)]
fn window_hwnd(frame: &eframe::Frame) -> Option<i64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as i64),
        _ => None,
    }
}

#[cfg(not(windows))]
fn window_hwnd(_frame: &eframe::Frame) -> Option<i64> {
    None
}

impl eframe::App for PuppyApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.perf.on_frame(frame);
        self.sup.drain();
        self.poll_folder_pick();

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
        let mut open_browser = false;
        let mut open_mcp = false;
        let mut pick_theme: Option<Theme> = None;
        let mut open_editor = false;
        let theme = self.theme.clone();
        let browser_available = self.browser.is_available();

        // Copy the bits the menu needs so its closure doesn't borrow `self`.
        let mut perf_visible = self.perf.visible;
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
        if open_browser {
            let id = self.browser.open_tab(None, None);
            if let Some(dock) = self.dock.as_mut() {
                dock.push_to_focused_leaf(Tab::Browser(id));
            }
        }
        if open_mcp && let Some(dock) = self.dock.as_mut() {
            // One instance: focus the existing tab if it's already open.
            if let Some(path) = dock.find_tab_from(|t| matches!(t, Tab::McpManager)) {
                let _ = dock.set_active_tab(path);
            } else {
                dock.push_to_focused_leaf(Tab::McpManager);
            }
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

        let mut dock = self.dock.take().expect("dock present");
        {
            let mut shell = Shell {
                sup: &mut self.sup,
                browser: &mut self.browser,
                mcp: &mut self.mcp,
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
