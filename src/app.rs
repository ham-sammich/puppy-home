//! Top-level app: supervises Code Puppy workspaces and hosts the dockable shell.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui_dock::{DockArea, DockState, Style};

use crate::shell::{Shell, ShellAction, Tab};
use crate::supervisor::Supervisor;

pub struct PuppyApp {
    sup: Supervisor,
    dock: Option<DockState<Tab>>,
    status: String,
    /// A folder-picker dialog runs on a worker thread; its result arrives here.
    /// (Running rfd inline would re-enter the egui frame and crash on Windows.)
    folder_pick: Option<Receiver<Option<PathBuf>>>,
}

impl PuppyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::configure(&cc.egui_ctx);
        // Make panel splitters (e.g. the editor/chat divider) easy to grab.
        cc.egui_ctx.style_mut(|s| {
            s.interaction.resize_grab_radius_side = 10.0;
            s.interaction.resize_grab_radius_corner = 14.0;
        });
        let mut sup = Supervisor::new(cc.egui_ctx.clone());
        let mut dock = DockState::new(vec![Tab::Dashboard]);
        let mut status = "Open a folder to start a Code Puppy workspace.".to_string();

        // Dev convenience: auto-open a workspace folder on launch.
        if let Ok(path) = std::env::var("PUPPY_HOME_OPEN") {
            match sup.open(PathBuf::from(&path)) {
                Ok(id) => {
                    dock.push_to_focused_leaf(Tab::Chat(id));
                    status.clear();
                }
                Err(e) => status = format!("Couldn't open {path}: {e}"),
            }
        }

        PuppyApp {
            sup,
            dock: Some(dock),
            status,
            folder_pick: None,
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
                    if let Some(dock) = self.dock.as_mut() {
                        dock.retain_tabs(|t| !matches!(t, Tab::Chat(x) if *x == id));
                    }
                }
                ShellAction::FocusChat(id) => {
                    if let Some(dock) = self.dock.as_mut() {
                        if let Some(path) =
                            dock.find_tab_from(|t| matches!(t, Tab::Chat(x) if *x == id))
                        {
                            let _ = dock.set_active_tab(path);
                        }
                    }
                }
                ShellAction::ShowChanges(id) => {
                    if let Some(ws) = self.sup.get_mut(id) {
                        ws.show_changes();
                    }
                    if let Some(dock) = self.dock.as_mut() {
                        if let Some(path) =
                            dock.find_tab_from(|t| matches!(t, Tab::Chat(x) if *x == id))
                        {
                            let _ = dock.set_active_tab(path);
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for PuppyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.sup.drain();
        self.poll_folder_pick();

        let mut actions: Vec<ShellAction> = Vec::new();
        let mut open_clicked = false;

        // Copy the bits the menu needs so its closure doesn't borrow `self`.
        let picking = self.folder_pick.is_some();
        let ws_count = self.sup.len();
        let waiting = self.sup.waiting_count();
        let status = self.status.clone();

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
            });
        });

        if open_clicked {
            self.begin_folder_pick(ui.ctx());
        }

        let mut dock = self.dock.take().expect("dock present");
        {
            let mut shell = Shell {
                sup: &mut self.sup,
                actions: &mut actions,
            };
            DockArea::new(&mut dock)
                .style(Style::from_egui(ui.style().as_ref()))
                .show_inside(ui, &mut shell);
        }
        self.dock = Some(dock);

        self.apply_actions(actions);

        // Keep elapsed timers ticking while any instance is busy.
        if self.sup.any_busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
