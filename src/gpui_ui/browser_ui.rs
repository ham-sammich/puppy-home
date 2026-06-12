//! The browser-plugin host surface — egui's dockable `Browser` tab
//! (`src/browser/mod.rs` render methods) at parity, dressed in the GPUI
//! tokens: the Install panel when the plugin isn't there, else the stdin
//! toolbar (back/forward/reload/DevTools/CDP/URL bar) driving the
//! supervised `puppy-browser` process. Also owns the dashboard's
//! installed-plugins section (E9).
//!
//! EMBEDDING IS N/A IN THE GPUI SHELL at this pin, on every OS: the
//! Windows path reparents into the egui HWND, and the macOS overlay glues
//! to the eframe viewport's inner_rect — neither attaches to the GPUI
//! window yet. Instead the drain loop's `float_pump` sends the plugin the
//! `float` command once it's ready, turning its (initially HIDDEN —
//! that was the E8 macOS bug) borderless window into a real decorated
//! floating window; the viewport region says so honestly.

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::browser::{BrowserManager, EmbedMode, NavOp, PluginStatus};
use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::managers_ui::{disabled_btn, small};
use crate::gpui_ui::widgets;
use crate::gpui_ui::{DashAction, RootView, Screen, Tokens};

/// Browser-surface interactions, nested under `DashAction::Browser`.
#[derive(Clone, Copy, Debug)]
pub enum BrowserAction {
    /// Toolbar button: open (lazily creating) the browser surface.
    Open,
    Nav(NavOp),
    /// URL bar submitted.
    Go,
    Launch,
    Stop,
    /// \u{2197} — pop the embedded webview out into a decorated floating
    /// window.
    PopOut,
    /// \u{2913} — re-embed the floating webview into the Browser screen.
    PopIn,
    /// \u{2715} on the Web tab: stop the plugin and dismiss the surface
    /// entirely (the tab previously could never be closed).
    CloseSurface,
    InstallLocal,
    Rescan,
    OpenPluginsDir,
    CopyCdp,
    /// Dashboard plugins-section header click.
    PluginsToggle,
}

impl RootView {
    pub(crate) fn dispatch_browser(&mut self, action: BrowserAction, cx: &mut gpui::Context<Self>) {
        match action {
            BrowserAction::Open => {
                let id = *self
                    .browser_tab
                    .get_or_insert_with(|| self.browser.open_tab(None, None));
                if self.browser_url_input.is_none() {
                    let entity = cx.new(|cx| ChatInput::new("Enter a URL\u{2026}", cx));
                    let sub = cx.subscribe(
                        &entity,
                        |this: &mut Self, _, ev: &crate::gpui_ui::InputEvent, cx| {
                            if matches!(ev, crate::gpui_ui::InputEvent::Submitted) {
                                this.dispatch_browser(BrowserAction::Go, cx);
                            }
                            cx.notify();
                        },
                    );
                    self.browser_url_input = Some(entity);
                    self.chat_subs.push(sub);
                }
                // Seed the bar with the tab's URL (surface reopened later).
                if let (Some(input), Some(url)) =
                    (&self.browser_url_input, self.browser.tab_url(id))
                {
                    input.update(cx, |i, cx| i.set_text(url, cx));
                }
                self.screen = Screen::Browser;
            }
            BrowserAction::Nav(op) => {
                if let Some(id) = self.browser_tab {
                    self.browser.nav(id, op);
                }
            }
            BrowserAction::Go | BrowserAction::Launch => {
                // Wake ticker (started once): the drain loop is event-driven,
                // but overlay discipline (hide-on-minimize) needs it to
                // breathe even when nothing else generates events.
                if !self.browser_ticker {
                    self.browser_ticker = true;
                    let waker = self.waker.clone();
                    std::thread::spawn(move || {
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(700));
                            waker.wake();
                        }
                    });
                }
                let Some(id) = self.browser_tab else { return };
                let text = self
                    .browser_url_input
                    .as_ref()
                    .map(|i| i.read(cx).text().to_string())
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    if matches!(action, BrowserAction::Launch) {
                        self.browser.launch_tab(id); // example.com fallback
                    }
                } else {
                    self.browser.navigate_to(id, &text);
                }
                // Reflect normalization (egui rewrites the field too).
                if let (Some(input), Some(url)) =
                    (&self.browser_url_input, self.browser.tab_url(id))
                {
                    input.update(cx, |i, cx| i.set_text(url, cx));
                }
            }
            BrowserAction::Stop => {
                if let Some(id) = self.browser_tab {
                    self.browser.stop_tab(id);
                }
            }
            BrowserAction::CloseSurface => {
                if let Some(id) = self.browser_tab.take() {
                    self.browser.stop_tab(id);
                    self.browser.close_tab(id);
                }
                *self.browser_embed_slot.lock().unwrap() = None;
                if self.screen == crate::gpui_ui::Screen::Browser {
                    self.screen = crate::gpui_ui::Screen::Dashboard;
                }
            }
            BrowserAction::PopOut => {
                if let Some(id) = self.browser_tab {
                    self.browser.set_tab_mode(id, EmbedMode::Floating);
                }
            }
            BrowserAction::PopIn => {
                if let Some(id) = self.browser_tab {
                    self.browser.set_tab_mode(id, EmbedMode::Embedded);
                }
            }
            BrowserAction::InstallLocal => self.browser.install_local(),
            BrowserAction::Rescan => self.browser.rescan(),
            BrowserAction::OpenPluginsDir => self.browser.open_plugins_folder(),
            BrowserAction::CopyCdp => {
                if let Some(url) = self.browser_tab.and_then(|id| self.browser.tab_cdp_url(id)) {
                    let accent = self.tokens.accent;
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url));
                    self.toast(
                        "CDP endpoint copied \u{2014} paste it to Code Puppy".into(),
                        accent,
                    );
                }
            }
            BrowserAction::PluginsToggle => self.plugins_open = !self.plugins_open,
        }
        cx.notify();
    }

    /// The browser screen body (install panel or toolbar + viewport note).
    pub(crate) fn browser_body(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let t = self.tokens;
        let root = cx.entity();
        if !self.browser.is_available() {
            return install_panel(&t, &root, &self.browser);
        }
        let Some(id) = self.browser_tab else {
            return div().into_any_element(); // Open always seeds the tab
        };
        let running = self.browser.tab_running(id);
        let cdp = self.browser.tab_cdp_url(id);
        let launch_error = self.browser.tab_launch_error(id);

        let nav_btn = |label: &str, tip: &str, op: NavOp, btn_id: &'static str| {
            if running {
                widgets::btn(&t, label)
                    .id(btn_id)
                    .tooltip(widgets::text_tip(tip.into()))
                    .on_click(bact(&root, BrowserAction::Nav(op)))
                    .into_any_element()
            } else {
                disabled_btn(&t, label).into_any_element()
            }
        };

        let toolbar = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(nav_btn("\u{2039}", "Back", NavOp::Back, "web-back"))
            .child(nav_btn("\u{203a}", "Forward", NavOp::Forward, "web-fwd"))
            .child(nav_btn("\u{21bb}", "Reload", NavOp::Reload, "web-reload"))
            .child(div().w(px(1.)).h(px(18.)).bg(t.line_soft))
            .child(nav_btn(
                "DevTools",
                "Open browser DevTools (F12)",
                NavOp::DevTools,
                "web-devtools",
            ))
            .children(cdp.map(|url| {
                widgets::btn(&t, "CDP")
                    .id("web-cdp")
                    .tooltip(widgets::text_tip(format!(
                        "Copy DevTools Protocol endpoint {url} \u{2014} paste it to \
                         Code Puppy to let it inspect this page"
                    )))
                    .on_click(bact(&root, BrowserAction::CopyCdp))
            }))
            .children(self.browser_url_input.as_ref().map(|input| {
                div()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded(px(8.))
                    .bg(t.well)
                    .border_1()
                    .border_color(t.line_soft)
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .child(input.clone())
            }))
            .children(running.then(|| {
                // Pop-out / pop-in toggle (the embedded overlay vs a real
                // decorated window).
                let id = self.browser_tab.unwrap_or_default();
                match self.browser.tab_mode(id) {
                    EmbedMode::Embedded => widgets::btn(&t, "\u{2197}")
                        .id("web-popout")
                        .tooltip(widgets::text_tip(
                            "Pop the browser out into its own window".into(),
                        ))
                        .on_click(bact(&root, BrowserAction::PopOut)),
                    EmbedMode::Floating => widgets::btn(&t, "\u{2913}")
                        .id("web-popin")
                        .tooltip(widgets::text_tip(
                            "Bring the browser back into this tab".into(),
                        ))
                        .on_click(bact(&root, BrowserAction::PopIn)),
                }
            }))
            .child(if running {
                widgets::btn(&t, "Stop")
                    .id("web-stop")
                    .tooltip(widgets::text_tip("Quit the browser process".into()))
                    .on_click(bact(&root, BrowserAction::Stop))
                    .into_any_element()
            } else {
                widgets::primary_btn(&t, "Launch")
                    .id("web-launch")
                    .on_click(bact(&root, BrowserAction::Launch))
                    .into_any_element()
            });

        let viewport: AnyElement = if running {
            let id = self.browser_tab.unwrap_or_default();
            match self.browser.tab_mode(id) {
                EmbedMode::Embedded => {
                    // The overlay window covers this region; the canvas
                    // records its bounds (window coords, logical px) for
                    // the render-upkeep embed pump. The note shows through
                    // until the plugin window lands on top of it.
                    let slot = self.browser_embed_slot.clone();
                    div()
                        .flex_1()
                        .rounded(px(10.))
                        .bg(t.well)
                        .child(
                            gpui::canvas(
                                move |bounds, _, _| {
                                    *slot.lock().unwrap() = Some((
                                        f32::from(bounds.origin.x),
                                        f32::from(bounds.origin.y),
                                        f32::from(bounds.size.width),
                                        f32::from(bounds.size.height),
                                    ));
                                },
                                |_, _, _, _| {},
                            )
                            .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(t.weak)
                                        .child("starting browser\u{2026}"),
                                ),
                        )
                        .into_any_element()
                }
                EmbedMode::Floating => center_note(
                    &t,
                    "Popped out \u{2014} the page lives in the \"Puppy Browser\" \
                     window. \u{2913} brings it back into this tab.",
                ),
            }
        } else {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .rounded(px(10.))
                .bg(t.well)
                .child(
                    widgets::primary_btn(&t, "Launch browser")
                        .id("web-launch-center")
                        .on_click(bact(&root, BrowserAction::Launch)),
                )
                .children(
                    launch_error
                        .clone()
                        .map(|e| small(&t, e, t.error).into_any_element()),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(toolbar)
            .children(
                (running && launch_error.is_some())
                    .then(|| small(&t, launch_error.unwrap_or_default(), t.error)),
            )
            .child(viewport)
            .into_any_element()
    }
}

/// Click handler funneling a browser action through the root dispatch.
fn bact(
    root: &Entity<RootView>,
    a: BrowserAction,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    let root = root.clone();
    move |_, _, cx| {
        root.update(cx, |r, cx| r.dispatch(DashAction::Browser(a), cx));
    }
}

/// Centered hint filling the viewport region (egui `viewport_note`).
fn center_note(t: &Tokens, text: &str) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .bg(t.well)
        .child(
            div()
                .text_size(px(13.))
                .text_color(t.weak)
                .whitespace_normal()
                .child(text.to_string()),
        )
        .into_any_element()
}

/// The "plugin not installed" panel (egui `render_install` at parity).
fn install_panel(t: &Tokens, root: &Entity<RootView>, browser: &BrowserManager) -> AnyElement {
    let status: AnyElement = match browser.plugin_status() {
        PluginStatus::NotFound => small(t, "Status: not found.", t.text).into_any_element(),
        PluginStatus::Incompatible { version, needs } => small(
            t,
            format!("Found v{version} but it needs a newer host (requires host \u{2265} {needs})."),
            t.paused,
        )
        .into_any_element(),
        PluginStatus::ExeMissing { exe } => small(
            t,
            format!("Manifest found but executable is missing: {exe}"),
            t.paused,
        )
        .into_any_element(),
        PluginStatus::Ready => small(t, "Status: ready.", t.text).into_any_element(),
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::BOLD)
                .text_color(t.text)
                .child("Browser plugin not installed"),
        )
        .child(small(
            t,
            "The in-app browser is an optional plugin so the base app stays small.",
            t.weak,
        ))
        .child(status)
        .children(BrowserManager::local_build_available().then(|| {
            widgets::primary_btn(t, "Install from local build")
                .id("web-install")
                .tooltip(widgets::text_tip(
                    "Copy the freshly-built puppy-browser into the plugins folder".into(),
                ))
                .on_click(bact(root, BrowserAction::InstallLocal))
        }))
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(
                    widgets::btn(t, "Open plugins folder")
                        .id("web-open-dir")
                        .on_click(bact(root, BrowserAction::OpenPluginsDir)),
                )
                .child(
                    widgets::btn(t, "Rescan")
                        .id("web-rescan")
                        .on_click(bact(root, BrowserAction::Rescan)),
                ),
        )
        .children(browser.plugins_dir().map(|dir| {
            div()
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .text_color(t.dim)
                .child(dir.display().to_string())
        }))
        .children(
            browser
                .install_error()
                .map(|e| small(t, e.to_string(), t.error)),
        )
        .into_any_element()
}

/// E9: the dashboard's collapsed installed-plugins list (egui
/// `plugins_section`): name + dir tooltip, version, status label/dot.
pub(crate) fn plugins_section(
    t: &Tokens,
    root: &Entity<RootView>,
    browser: &BrowserManager,
    open: bool,
) -> AnyElement {
    let plugins = browser.plugins();
    let header = div()
        .id("plugins-header")
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.weak)
        .child(if open { "\u{25be}" } else { "\u{25b8}" })
        .child(format!("Plugins ({})", plugins.len()))
        .on_click(bact(root, BrowserAction::PluginsToggle));

    let mut section = div().flex().flex_col().gap_1().child(header);
    if open {
        if plugins.is_empty() {
            section = section.child(small(
                t,
                "No plugins installed. Open the Browser tab to install one.",
                t.dim,
            ));
        }
        for (i, p) in plugins.iter().enumerate() {
            let (label, color) = if p.is_runnable() {
                ("ready", gpui::rgb(0x78c88c))
            } else if !p.manifest.is_compatible() {
                ("incompatible", t.paused)
            } else {
                ("exe missing", t.error)
            };
            section = section.child(
                div()
                    .id(("plugin-row", i as u64))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(8.)).rounded_full().bg(color))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(p.manifest.name.clone()),
                    )
                    .child(small(t, format!("v{}", p.manifest.version), t.dim))
                    .child(small(t, label, color))
                    .tooltip(widgets::text_tip(p.dir.display().to_string())),
            );
        }
    }
    section.into_any_element()
}
