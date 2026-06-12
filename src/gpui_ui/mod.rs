//! The GPUI frontend: root view, drain loop, and the Command Center
//! dashboard (Task 2.2).
//!
//! ## Patterns (documented in GPUI_NOTES.md — the template for Phase 2)
//! - **One entity**: `RootView` owns the `Supervisor` plus all dashboard UI
//!   state (view mode, toasts, the open inline input, the open popover).
//! - **Snapshots down, actions up**: `render` builds plain `CardSnapshot`
//!   structs and feeds `RenderOnce` components; handlers funnel back through
//!   `Entity<RootView>::update → dispatch(DashAction)` — a single mutation
//!   choke point, mirroring the egui branch's `ShellAction` queue.
//! - **The drain loop** (Task 2.1) stays the only repaint driver: backend
//!   wakes or the adaptive timer → `Supervisor::drain()` → `cx.notify()`.

pub mod assets;
pub mod dashboard;
pub mod tokens;
pub mod waker;
pub mod widgets;

use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Application, Bounds, Context, FocusHandle, FontWeight, IntoElement, Keystroke,
    ParentElement as _, Rgba, SharedString, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

use crate::session::{self, DashboardViewMode};
use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceId;
use dashboard::{CardInput, InputKind};
use tokens::Tokens;
use waker::GpuiWaker;
use widgets::{Toast, alpha};

/// Drain cadence while at least one workspace is mid-turn.
const DRAIN_BUSY: Duration = Duration::from_millis(250);
/// Drain cadence when the whole fleet is idle (wakes still land instantly).
const DRAIN_IDLE: Duration = Duration::from_millis(1000);

/// Launch the GPUI app (the default `main` on this branch).
pub fn run() {
    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            assets::register_fonts(cx);
            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Code Puppy")),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(options, |_, cx| cx.new(RootView::new))
                .expect("failed to open the main window");
            cx.activate(true);
        });
}

/// Where a card asked to navigate. The chat / diff views land in Task 2.3;
/// until then the intent is recorded + surfaced as a toast (honest stub).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavIntent {
    Chat(WorkspaceId),
    Changes(WorkspaceId),
}

/// Every dashboard interaction, funneled through [`RootView::dispatch`].
#[derive(Clone, Debug)]
pub enum DashAction {
    Pause(WorkspaceId),
    Resume(WorkspaceId),
    Stop(WorkspaceId),
    Restart(WorkspaceId),
    SetModel(WorkspaceId, String),
    /// Submit the open inline input (steer / new prompt).
    SubmitInput,
    CloseInput,
    /// Flip the open steer input's delivery mode (false = now, true = queue).
    SetSteerQueue(bool),
    TogglePopover(WorkspaceId),
    ClosePopover,
    Open(WorkspaceId),
    Changes(WorkspaceId),
    SetView(DashboardViewMode),
    ToggleMotion,
}

pub struct RootView {
    supervisor: Supervisor,
    tokens: Tokens,
    /// Most recent open/spawn error, shown inline.
    last_error: Option<String>,
    /// `PUPPY_GPUI_PROMPT`: one-shot probe prompt (Task 2.1 instrumentation).
    probe_prompt: Option<String>,
    // -- dashboard state --
    dash_mode: DashboardViewMode,
    reduce_motion: bool,
    toasts: Vec<Toast>,
    card_input: Option<CardInput>,
    model_popover: Option<WorkspaceId>,
    input_focus: FocusHandle,
    /// Last navigation ask (chat/diff land in 2.3); kept for wiring + tests.
    pub pending_nav: Option<NavIntent>,
}

impl RootView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (waker, wake_rx) = GpuiWaker::new();
        let mut supervisor = Supervisor::new(waker);
        let mut last_error = None;

        if let Some(root) = std::env::var_os("PUPPY_GPUI_OPEN") {
            if let Err(e) = supervisor.open(root.into()) {
                last_error = Some(e);
            }
        }

        // Shared prefs: same session.json fields as the egui branch.
        let saved = session::load();

        Self::spawn_drain_loop(cx, wake_rx);
        RootView {
            supervisor,
            tokens: Tokens::dark(),
            last_error,
            probe_prompt: std::env::var("PUPPY_GPUI_PROMPT").ok(),
            dash_mode: saved.dashboard_view,
            reduce_motion: saved.reduce_motion,
            toasts: Vec::new(),
            card_input: None,
            model_popover: None,
            input_focus: cx.focus_handle(),
            pending_nav: None,
        }
    }

    /// The recurring drain task: wake-driven with an adaptive timer floor.
    fn spawn_drain_loop(cx: &mut Context<Self>, mut wake_rx: UnboundedReceiver<()>) {
        let probe = std::env::var_os("PUPPY_GPUI_PROBE").is_some();
        let mut last_probe = String::new();
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(busy) = this.update(cx, |root, cx| {
                    root.supervisor.drain();
                    root.maybe_send_probe_prompt();
                    root.prune_toasts();
                    cx.notify();
                    if probe {
                        let line = root.probe_line();
                        if line != last_probe {
                            eprintln!("[probe] {line}");
                            last_probe = line;
                        }
                    }
                    // Toasts need ticks to expire; animations repaint on
                    // their own, but the elapsed clocks ride the busy cadence.
                    root.supervisor.any_busy() || !root.toasts.is_empty()
                }) else {
                    return;
                };

                let cadence = if busy { DRAIN_BUSY } else { DRAIN_IDLE };
                let timer = cx.background_executor().timer(cadence);
                futures::select_biased! {
                    _ = wake_rx.next() => {}
                    _ = futures::FutureExt::fuse(timer) => {}
                }
                while let Ok(()) = wake_rx.try_recv() {}
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    /// The single mutation funnel for every dashboard interaction.
    pub fn dispatch(&mut self, action: DashAction, cx: &mut Context<Self>) {
        let accent = self.tokens.accent;
        let (run, paused_c, error_c) = (self.tokens.run, self.tokens.paused, self.tokens.error);
        match action {
            DashAction::Pause(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.pause_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} paused at next safe point"), paused_c);
                }
            }
            DashAction::Resume(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.resume_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} resumed"), run);
                }
            }
            DashAction::Stop(id) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.stop_turn();
                    let name = ws.name.clone();
                    self.toast(format!("{name} stopped"), error_c);
                }
            }
            DashAction::Restart(id) => {
                let name = self.ws_name(id);
                self.supervisor.restart(id);
                self.toast(format!("Restarting {name}\u{2026}"), run);
            }
            DashAction::SetModel(id, model) => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.set_model_live(&model);
                    let name = ws.name.clone();
                    self.toast(format!("{name} \u{2192} {model}"), accent);
                }
                self.model_popover = None;
            }
            DashAction::SubmitInput => self.submit_input(),
            DashAction::CloseInput => self.card_input = None,
            DashAction::SetSteerQueue(q) => {
                if let Some(input) = &mut self.card_input {
                    input.queue = q;
                }
            }
            DashAction::TogglePopover(id) => {
                self.model_popover = if self.model_popover == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }
            DashAction::ClosePopover => self.model_popover = None,
            DashAction::Open(id) => {
                let name = self.ws_name(id);
                self.pending_nav = Some(NavIntent::Chat(id));
                eprintln!("[nav] open chat for workspace {} ({name})", id.0);
                self.toast(
                    format!("Opening {name}\u{2026} (chat view lands in Task 2.3)"),
                    accent,
                );
            }
            DashAction::Changes(id) => {
                let name = self.ws_name(id);
                self.pending_nav = Some(NavIntent::Changes(id));
                eprintln!("[nav] open changes for workspace {} ({name})", id.0);
                self.toast(
                    format!("Opening {name} changes\u{2026} (diff view lands in Task 2.3)"),
                    accent,
                );
            }
            DashAction::SetView(mode) => {
                self.dash_mode = mode;
                self.save_prefs();
            }
            DashAction::ToggleMotion => {
                self.reduce_motion = !self.reduce_motion;
                self.save_prefs();
                let state = if self.reduce_motion { "off" } else { "on" };
                self.toast(format!("Decorative motion {state}"), accent);
            }
        }
        cx.notify();
    }

    /// Toggle a card's inline input (one open card at a time) + focus it.
    pub fn toggle_input(
        &mut self,
        ws: WorkspaceId,
        kind: InputKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = matches!(&self.card_input, Some(i) if i.ws == ws && i.kind == kind);
        self.card_input = if open {
            None
        } else {
            window.focus(&self.input_focus);
            Some(CardInput {
                ws,
                kind,
                text: String::new(),
                queue: false,
            })
        };
        cx.notify();
    }

    /// Minimal inline-input editing: printable chars, backspace, cmd-V paste.
    /// (The full IME-aware text input arrives with the 2.3 composer.)
    pub fn edit_input(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let paste = ks.modifiers.platform && ks.key == "v";
        let clip = paste
            .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
            .flatten();
        let Some(input) = &mut self.card_input else {
            return;
        };
        if ks.key == "backspace" {
            input.text.pop();
        } else if let Some(text) = clip {
            input.text.push_str(&text);
        } else if !ks.modifiers.platform
            && !ks.modifiers.control
            && let Some(ch) = &ks.key_char
        {
            input.text.push_str(ch);
        } else {
            return;
        }
        cx.notify();
    }

    fn submit_input(&mut self) {
        let Some(input) = self.card_input.take() else {
            return;
        };
        let text = input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let accent = self.tokens.accent;
        let Some(ws) = self.supervisor.get_mut(input.ws) else {
            return;
        };
        let name = ws.name.clone();
        match input.kind {
            InputKind::Steer => {
                if ws.steer_text(&text, input.queue) {
                    let how = if input.queue {
                        "(queued \u{1f4e8})"
                    } else {
                        "now \u{1f3af}"
                    };
                    self.toast(format!("Steered {name} {how}"), accent);
                } else {
                    self.toast(
                        format!("{name} isn't running \u{2014} steer dropped"),
                        accent,
                    );
                }
            }
            InputKind::Send => {
                ws.send_prompt_text(&text);
                self.toast(format!("Sent {name}"), accent);
            }
        }
    }

    fn toast(&mut self, msg: String, color: Rgba) {
        self.toasts.push(Toast {
            msg,
            color,
            born: Instant::now(),
        });
    }

    fn prune_toasts(&mut self) {
        let now = Instant::now();
        self.toasts
            .retain(|t| now.duration_since(t.born) < widgets::TOAST_TTL);
    }

    fn ws_name(&self, id: WorkspaceId) -> String {
        self.supervisor
            .get(id)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| format!("workspace {}", id.0))
    }

    /// Read-modify-write the shared session.json (preserves the egui
    /// branch's fields — the two apps share one config).
    fn save_prefs(&self) {
        let mut s = session::load();
        s.dashboard_view = self.dash_mode;
        s.reduce_motion = self.reduce_motion;
        session::save(&s);
    }

    /// The puppy's name as reported by any sidecar ("Puppy" until one is).
    fn puppy_name(&self) -> String {
        self.supervisor
            .iter()
            .map(|w| w.puppy_name.as_str())
            .find(|n| !n.is_empty() && *n != "Puppy")
            .unwrap_or("Puppy")
            .to_string()
    }

    /// Fire the probe prompt once the first sidecar reports ready.
    fn maybe_send_probe_prompt(&mut self) {
        let Some(prompt) = &self.probe_prompt else {
            return;
        };
        let Some(id) = self.supervisor.iter().find(|w| w.is_ready()).map(|w| w.id) else {
            return;
        };
        let prompt = prompt.clone();
        if let Some(ws) = self.supervisor.get_mut(id) {
            eprintln!("[probe] sending prompt to {}: {prompt:?}", ws.name);
            ws.send_prompt_text(&prompt);
            self.probe_prompt = None;
        }
    }

    /// One-line fleet summary for the PUPPY_GPUI_PROBE log.
    fn probe_line(&self) -> String {
        if self.supervisor.is_empty() {
            return "no workspaces".to_string();
        }
        self.supervisor
            .iter()
            .map(|w| {
                format!(
                    "{}: {} tok={} rate={:.1}/s [{}]",
                    w.name,
                    w.status.label(),
                    w.total_tokens,
                    w.token_rate,
                    w.status_line
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// Native folder picker → spawn a sidecar for the chosen root.
    fn open_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut picked))) = paths.await
                && let Some(root) = picked.pop()
            {
                let _ = this.update(cx, |root_view, cx| {
                    match root_view.supervisor.open(root) {
                        Ok(_) => root_view.last_error = None,
                        Err(e) => root_view.last_error = Some(e),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Toolbar: Open Folder, reduce-motion toggle, view segmented control.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.tokens;
        let entity = cx.entity();
        let stats_sub = {
            let s = dashboard::fleet_stats(&self.supervisor);
            format!("{} agents running \u{b7} {:.0} tok/s", s.running, s.tps)
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child("\u{1f43e} Code Puppy"),
            )
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(t.card)
                    .border_1()
                    .border_color(t.line_soft)
                    .text_size(px(11.5))
                    .text_color(t.text)
                    .child(format!("\u{1f436} {}", self.puppy_name())),
            )
            .child(
                widgets::btn(t, "\u{1f4c1} Open Folder\u{2026}")
                    .id("open-folder")
                    .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.5))
                    .font_family("JetBrains Mono")
                    .text_color(t.weak)
                    .child(stats_sub),
            )
            .child(
                widgets::btn(
                    t,
                    if self.reduce_motion {
                        "Motion: off"
                    } else {
                        "Motion: on"
                    },
                )
                .id("motion-toggle")
                .when(self.reduce_motion, |d| {
                    d.border_color(alpha(self.tokens.paused, 0.7))
                })
                .on_click(
                    cx.listener(|this, _, _, cx| this.dispatch(DashAction::ToggleMotion, cx)),
                ),
            )
            .child(dashboard::segmented(t, self.dash_mode, &entity))
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.tokens;
        let entity = cx.entity();
        let puppy = self.puppy_name();
        let stats = dashboard::fleet_stats(&self.supervisor);
        let agg = self.supervisor.aggregate_sparks().to_vec();

        // Snapshots: catalog only for the card whose popover is open.
        let cards: Vec<dashboard::CardSnapshot> = self
            .supervisor
            .iter()
            .map(|ws| dashboard::snapshot(ws, &t, self.model_popover == Some(ws.id)))
            .collect();
        let waiting: Vec<(WorkspaceId, String, Option<String>)> = self
            .supervisor
            .iter()
            .filter(|w| w.status == crate::workspace::InstanceStatus::WaitingForInput)
            .map(|w| (w.id, w.name.clone(), w.pending_question().map(String::from)))
            .collect();
        let input_snap = self
            .card_input
            .as_ref()
            .map(|i| (i.ws, i.kind, i.text.clone(), i.queue));
        let empty = cards.is_empty();

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(13.))
            .font_family("Space Grotesk")
            .child(self.toolbar(cx))
            .children(
                self.last_error
                    .clone()
                    .map(|e| div().text_size(px(12.)).text_color(t.error).child(e)),
            )
            .child(
                div()
                    .id("dash-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(dashboard::pack_header(&t, &puppy, &stats, agg))
                            .child(dashboard::attention_banner(&t, &waiting, &entity))
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .py_12()
                                        .flex()
                                        .justify_center()
                                        .text_color(t.weak)
                                        .child(format!(
                                            "No agents running. Open a folder to send {puppy} out. \u{1f43e}"
                                        )),
                                )
                            })
                            .when(!empty, |d| {
                                d.child(dashboard::fleet(
                                    &t,
                                    self.dash_mode,
                                    cards,
                                    &entity,
                                    input_snap,
                                    &self.input_focus,
                                    self.reduce_motion,
                                ))
                            }),
                    ),
            )
            .child(widgets::toast_layer(&t, &self.toasts))
    }
}
