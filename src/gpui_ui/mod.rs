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

pub mod actions;
pub mod assets;
pub mod chat;
pub mod dashboard;
pub mod input;
pub mod markdown;
pub mod tokens;
pub mod waker;
pub mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Application, Bounds, Context, Entity, FocusHandle, Focusable as _, FontWeight,
    IntoElement, ParentElement as _, Rgba, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::session::{self, ComposerStyle, DashboardViewMode};
use crate::supervisor::Supervisor;
use crate::workspace::WorkspaceId;
pub use actions::{ChatPop, DashAction};
use dashboard::CardInput;
use input::{ChatInput, InputEvent};
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
            input::bind_keys(cx);
            cx.open_window(options, |_, cx| cx.new(RootView::new))
                .expect("failed to open the main window");
            cx.activate(true);
        });
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
    // -- chat state --
    /// `None` = Dashboard; `Some(id)` = that workspace's chat.
    screen: Option<WorkspaceId>,
    composer_style: ComposerStyle,
    chat_inputs: HashMap<WorkspaceId, Entity<ChatInput>>,
    chat_subs: Vec<gpui::Subscription>,
    chat_pop: Option<ChatPop>,
    /// Transcript entries with an opened collapsible body (diff / thinking).
    expanded_entries: HashSet<(u64, usize)>,
    /// Workspaces rendering the full transcript ("Show older" clicked).
    show_all_chat: HashSet<WorkspaceId>,
    /// Workspaces with the explorer hidden (default = shown).
    tree_closed: HashSet<WorkspaceId>,
    expanded_dirs: HashSet<(u64, PathBuf)>,
    /// Focus the chat input on the next render (set when a chat opens).
    pending_focus: Option<WorkspaceId>,
    /// `PUPPY_GPUI_SCREEN=chat`: auto-open the first ready workspace's chat
    /// (probe instrumentation, like PUPPY_GPUI_PROMPT).
    probe_chat_screen: bool,
    /// Shared single-line answer input (ask Other rows + input prompts).
    answer_input: Option<Entity<ChatInput>>,
    /// Which (workspace, ask-question) the answer input currently feeds.
    other_target: Option<(WorkspaceId, usize)>,
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
            screen: None,
            composer_style: saved.composer_style,
            chat_inputs: HashMap::new(),
            chat_subs: Vec::new(),
            chat_pop: None,
            expanded_entries: HashSet::new(),
            show_all_chat: HashSet::new(),
            tree_closed: HashSet::new(),
            expanded_dirs: HashSet::new(),
            pending_focus: None,
            probe_chat_screen: std::env::var("PUPPY_GPUI_SCREEN").as_deref() == Ok("chat"),
            answer_input: None,
            other_target: None,
        }
    }

    /// Create the shared answer input on demand (ask Other / input prompts).
    pub(crate) fn ensure_answer_input(&mut self, cx: &mut Context<Self>) {
        if self.answer_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("Type your answer\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if *event == InputEvent::Submitted {
                this.dispatch(DashAction::AnswerEnter, cx);
            }
        });
        self.answer_input = Some(entity);
        self.chat_subs.push(sub);
    }

    /// Eagerly create the answer input once something needs answering, so
    /// the panel can render it (entities aren't created during render).
    fn ensure_answer_input_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.answer_input.is_some() {
            return;
        }
        let needed = self.supervisor.iter().any(|w| {
            w.ask_state().is_some()
                || matches!(
                    w.pending_request().map(|p| &p.kind),
                    Some(crate::workspace::PendingKind::Input { .. })
                )
        });
        if needed {
            self.ensure_answer_input(cx);
        }
    }

    /// Probe: jump to the first ready workspace's chat once, if asked to.
    fn maybe_probe_chat_screen(&mut self, cx: &mut Context<Self>) {
        if !self.probe_chat_screen {
            return;
        }
        let Some(id) = self.supervisor.iter().find(|w| w.is_ready()).map(|w| w.id) else {
            return;
        };
        self.probe_chat_screen = false;
        self.dispatch(DashAction::Open(id), cx);
        eprintln!("[probe] opened chat screen for workspace {}", id.0);
    }

    /// Lazily create (and subscribe to) the composer input for a workspace.
    fn ensure_chat_input(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if self.chat_inputs.contains_key(&id) {
            return;
        }
        let puppy = self.puppy_name();
        let entity = cx.new(|cx| {
            ChatInput::new(
                format!("Message {puppy}\u{2026}  (enter sends, shift-enter newline)"),
                cx,
            )
        });
        let sub = cx.subscribe(
            &entity,
            move |this, input, event: &InputEvent, cx| match event {
                InputEvent::Edited => {
                    let text = input.read(cx).text().to_string();
                    if let Some(ws) = this.supervisor.get_mut(id) {
                        ws.update_completions(&text);
                    }
                    cx.notify();
                }
                InputEvent::Submitted => this.dispatch(DashAction::ChatSubmit(id), cx),
            },
        );
        self.chat_inputs.insert(id, entity);
        self.chat_subs.push(sub);
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
                    root.maybe_probe_chat_screen(cx);
                    root.ensure_answer_input_if_needed(cx);
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
        s.composer_style = self.composer_style;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.tokens;
        let entity = cx.entity();
        let puppy = self.puppy_name();

        // One-shot: focus the composer when a chat was just opened.
        if let Some(id) = self.pending_focus.take()
            && let Some(input) = self.chat_inputs.get(&id)
        {
            window.focus(&input.read(cx).focus_handle(cx));
        }

        // A closed workspace can leave `screen` dangling — fall back to dash.
        if let Some(id) = self.screen
            && self.supervisor.get(id).is_none()
        {
            self.screen = None;
        }

        let tabs: Vec<(WorkspaceId, String, Rgba)> = self
            .supervisor
            .iter()
            .map(|w| {
                (
                    w.id,
                    w.name.clone(),
                    dashboard::card_state(w.status, &t).color,
                )
            })
            .collect();
        let strip = chat::tab_strip(&t, tabs, self.screen, &entity);

        let body: gpui::AnyElement = match self.screen {
            Some(id) => {
                let ws = self.supervisor.get(id).expect("validated above");
                let input = self.chat_inputs.get(&id).expect("created on open").clone();
                chat::chat_screen(&chat::ChatArgs {
                    t,
                    ws,
                    root: entity.clone(),
                    input,
                    style: self.composer_style,
                    pop: self.chat_pop.as_ref(),
                    puppy: puppy.clone(),
                    show_all: self.show_all_chat.contains(&id),
                    expanded: &self.expanded_entries,
                    reduce_motion: self.reduce_motion,
                    tree_open: !self.tree_closed.contains(&id),
                    expanded_dirs: &self.expanded_dirs,
                    answer_input: self.answer_input.as_ref(),
                    other_target: self
                        .other_target
                        .filter(|(tid, _)| *tid == id)
                        .map(|(_, qi)| qi),
                })
            }
            None => self.dashboard_body(cx),
        };

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_2p5()
            .p_4()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(13.))
            .font_family("Space Grotesk")
            .child(self.toolbar(cx))
            .child(strip)
            .children(
                self.last_error
                    .clone()
                    .map(|e| div().text_size(px(12.)).text_color(t.error).child(e)),
            )
            .child(body)
            .child(widgets::toast_layer(&t, &self.toasts))
    }
}

impl RootView {
    /// The dashboard screen body (Task 2.2), extracted from `render`.
    fn dashboard_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
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
            .size_full()
            .flex()
            .flex_col()
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
            .into_any_element()
    }
}
