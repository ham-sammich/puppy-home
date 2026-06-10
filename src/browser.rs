//! The in-app browser, delivered as an optional plugin (Architecture "C").
//!
//! Increment 1 (this file) wires the *shell* around the plugin: a dockable
//! `Browser` tab that shows an **Install** panel when the plugin is missing and
//! an **Open** toolbar when it's present. Launching the standalone `wry`
//! companion process + overlaying its native window comes next; the seams for
//! it (per-tab state, the runnable check) are already here.

use std::collections::BTreeMap;

use eframe::egui;

use crate::plugin::PluginRegistry;

/// The plugin id this manager drives.
const BROWSER_PLUGIN_ID: &str = "browser";

/// Identifies one open browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrowserId(pub u64);

/// Per-tab browser state (URL bar, etc.). Process handles will join this later.
#[derive(Default)]
struct BrowserTab {
    url: String,
    /// Last navigation the user asked for (stub until the process exists).
    last_go: Option<String>,
}

/// Owns plugin discovery + the open browser tabs.
pub struct BrowserManager {
    registry: PluginRegistry,
    tabs: BTreeMap<BrowserId, BrowserTab>,
    next_id: u64,
}

impl BrowserManager {
    /// Build the manager, discovering installed plugins up front.
    pub fn discover() -> Self {
        BrowserManager {
            registry: PluginRegistry::discover(),
            tabs: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// Whether the browser plugin is installed *and* runnable on this host.
    pub fn is_available(&self) -> bool {
        self.registry
            .get(BROWSER_PLUGIN_ID)
            .map(|p| p.is_runnable())
            .unwrap_or(false)
    }

    /// Allocate a fresh tab id (and its backing state).
    pub fn new_tab(&mut self) -> BrowserId {
        let id = BrowserId(self.next_id);
        self.next_id += 1;
        self.tabs.insert(id, BrowserTab::default());
        id
    }

    /// Forget a closed tab (later: also shut down its process).
    pub fn close_tab(&mut self, id: BrowserId) {
        self.tabs.remove(&id);
    }

    /// A short title for the tab strip.
    pub fn tab_title(&self, _id: BrowserId) -> String {
        "Browser".to_string()
    }

    /// Render one browser tab: Install panel or the (stub) browser toolbar.
    pub fn render_tab(&mut self, ui: &mut egui::Ui, id: BrowserId) {
        if self.is_available() {
            self.render_browser(ui, id);
        } else {
            self.render_install(ui);
        }
    }

    /// The "plugin not installed" panel.
    fn render_install(&mut self, ui: &mut egui::Ui) {
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

            // Explain *why* it isn't available: missing vs incompatible vs no exe.
            match self.registry.get(BROWSER_PLUGIN_ID) {
                None => {
                    ui.label("Status: not found.");
                }
                Some(p) if !p.manifest.is_compatible() => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "Found v{} but it needs a newer host (requires \u{2265} host {}).",
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
            ui.label("To install, drop the plugin folder here:");
            match self.registry.dir() {
                Some(dir) => {
                    ui.horizontal(|ui| {
                        ui.code(dir.display().to_string());
                        if ui.small_button("Copy path").clicked() {
                            ui.ctx().copy_text(dir.display().to_string());
                        }
                    });
                }
                None => {
                    ui.weak("(could not resolve a plugins directory on this system)");
                }
            }

            ui.add_space(12.0);
            if ui.button("Rescan plugins").clicked() {
                self.registry.rescan();
            }
        });
    }

    /// The browser toolbar + viewport placeholder (real webview lands next).
    fn render_browser(&mut self, ui: &mut egui::Ui, id: BrowserId) {
        let plugin_name = self
            .registry
            .get(BROWSER_PLUGIN_ID)
            .map(|p| p.manifest.name.clone())
            .unwrap_or_else(|| "Browser".into());

        let tab = self.tabs.entry(id).or_default();

        // Toolbar.
        ui.horizontal(|ui| {
            // Nav buttons are stubs until the process backend exists.
            ui.add_enabled(false, egui::Button::new("\u{2039}")) // back
                .on_hover_text("Back (coming soon)");
            ui.add_enabled(false, egui::Button::new("\u{203a}")) // forward
                .on_hover_text("Forward (coming soon)");
            ui.add_enabled(false, egui::Button::new("\u{21bb}")) // reload
                .on_hover_text("Reload (coming soon)");

            let go = ui
                .add(
                    egui::TextEdit::singleline(&mut tab.url)
                        .hint_text("Enter a URL…")
                        .desired_width(f32::INFINITY),
                )
                .lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if go && !tab.url.trim().is_empty() {
                tab.last_go = Some(tab.url.trim().to_string());
            }
        });
        ui.separator();

        // Viewport placeholder. The standalone plugin's native window will be
        // positioned over exactly this rect in the next increment.
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        let center = rect.center();
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{plugin_name}\nwebview viewport will render here"),
            egui::FontId::proportional(14.0),
            ui.visuals().weak_text_color(),
        );
        if let Some(url) = &tab.last_go {
            painter.text(
                egui::pos2(center.x, center.y + 28.0),
                egui::Align2::CENTER_CENTER,
                format!("(would navigate to: {url})"),
                egui::FontId::monospace(12.0),
                ui.visuals().text_color(),
            );
        }
    }
}
