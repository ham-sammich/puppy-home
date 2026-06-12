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
pub mod den;
pub mod editor;
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
use crate::waker::UiWaker;
use crate::workspace::WorkspaceId;
pub use actions::{ChatPop, DashAction, TreeOp};
use dashboard::CardInput;
use den::{DenConn, DenPop, DenSeg, TaskTarget};
use input::{ChatInput, InputEvent};
use tokens::Tokens;
use waker::GpuiWaker;
use widgets::{Toast, alpha};

/// Which top-level screen the window shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Dashboard,
    Chat(WorkspaceId),
    Den,
}

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
    screen: Screen,
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
    // -- den state --
    /// Shared waker (PackClient connections need it).
    waker: std::sync::Arc<dyn UiWaker>,
    den: Option<DenConn>,
    den_seg: DenSeg,
    den_pop: Option<DenPop>,
    den_feed_input: Option<Entity<ChatInput>>,
    den_task_input: Option<Entity<ChatInput>>,
    den_task_target: Option<TaskTarget>,
    den_show_all_feed: bool,
    den_join_addr: Option<Entity<ChatInput>>,
    den_join_room: Option<Entity<ChatInput>>,
    den_join_user: Option<Entity<ChatInput>>,
    den_join_error: Option<String>,
    den_roster_at: Option<Instant>,
    den_roster_last: String,
    /// Presence heuristic inputs (unfocused OR >5min since interaction).
    window_active: bool,
    last_interaction: Instant,
    presence_idle: bool,
    /// Probe runs never write session.json (don't clobber the user's state).
    session_no_save: bool,
    /// Last saved workspace-list signature (change-gated drain saves).
    session_sig: String,
    /// Images pasted into a workspace's composer, awaiting the next send.
    pending_images: HashMap<WorkspaceId, Vec<PendingImage>>,
    /// Completion-palette selection (reset on edit / new completions).
    palette_sel: usize,
    /// Steer delivery mode for composer-dock steering (false = now).
    chat_steer_queue: bool,
    /// Workspaces with the sidecar-logs panel open.
    logs_open: HashSet<WorkspaceId>,
    /// The sessions browser overlay, when open for a workspace.
    sessions_open: Option<WorkspaceId>,
    /// Selected session in the browser `(name, source)`.
    session_selected: Option<(String, String)>,
    sessions_filter_input: Option<Entity<ChatInput>>,
    /// Thinking folds that are closed (turn-end auto-collapse + manual).
    collapsed_thinking: HashSet<(u64, usize)>,
    /// Code-editor input entities, one per open (workspace, file).
    editor_inputs: HashMap<(u64, PathBuf), Entity<ChatInput>>,
    /// Dirty-close confirmation: second click on this tab's X closes it.
    pub(crate) editor_close_confirm: Option<(WorkspaceId, usize)>,
    /// Active tree operation (rename / new file / new folder) + its input.
    pub(crate) tree_op: Option<TreeOp>,
    pub(crate) tree_op_input: Option<Entity<ChatInput>>,
    pub(crate) tree_delete_confirm: Option<(WorkspaceId, PathBuf, bool)>,
}

/// One pasted image: the wire form + the displayable form.
pub struct PendingImage {
    pub b64: String,
    pub img: std::sync::Arc<gpui::Image>,
}

impl RootView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (waker, wake_rx) = GpuiWaker::new();
        let waker: std::sync::Arc<dyn UiWaker> = waker;
        let mut supervisor = Supervisor::new(waker.clone());
        let mut last_error = None;

        // Shared prefs + workspaces: same session.json the egui shell writes.
        let saved = session::load();

        // Probe runs (PUPPY_GPUI_OPEN) are isolated: they neither restore the
        // saved workspaces nor write session.json (probe_no_save below).
        let probing = std::env::var_os("PUPPY_GPUI_OPEN").is_some();
        if let Some(root) = std::env::var_os("PUPPY_GPUI_OPEN") {
            if let Err(e) = supervisor.open(root.into()) {
                last_error = Some(e);
            }
        } else {
            // B10 session restore — exact egui semantics (app/mod.rs):
            // folders that moved/vanished since last run are skipped quietly.
            for entry in saved.workspaces.clone() {
                let path = std::path::PathBuf::from(&entry.path);
                if !path.is_dir() {
                    continue; // folder moved/deleted since last run
                }
                if let Ok(id) = supervisor.open(path)
                    && let Some(ws) = supervisor.get_mut(id)
                {
                    ws.set_restore(entry.agent, entry.model, entry.autosave);
                }
            }
        }

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
            screen: Screen::Dashboard,
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
            waker,
            den: None,
            den_seg: DenSeg::default(),
            den_pop: None,
            den_feed_input: None,
            den_task_input: None,
            den_task_target: None,
            den_show_all_feed: false,
            den_join_addr: None,
            den_join_room: None,
            den_join_user: None,
            den_join_error: None,
            den_roster_at: None,
            den_roster_last: String::new(),
            window_active: true,
            last_interaction: Instant::now(),
            presence_idle: false,
            session_no_save: probing,
            session_sig: String::new(),
            pending_images: HashMap::new(),
            palette_sel: 0,
            chat_steer_queue: false,
            logs_open: HashSet::new(),
            sessions_open: None,
            session_selected: None,
            sessions_filter_input: None,
            collapsed_thinking: HashSet::new(),
            editor_inputs: HashMap::new(),
            editor_close_confirm: None,
            tree_op: None,
            tree_op_input: None,
            tree_delete_confirm: None,
        }
    }

    /// Create (once) the code-editor input for an open file: seeded from the
    /// buffer, syntax-highlighted on load and on every edit (200 KB cap),
    /// Cmd/Ctrl+S routed to the save action.
    pub(crate) fn ensure_editor_input(
        &mut self,
        id: WorkspaceId,
        path: &PathBuf,
        cx: &mut Context<Self>,
    ) {
        let key = (id.0, path.clone());
        if self.editor_inputs.contains_key(&key) {
            return;
        }
        let content = self
            .supervisor
            .get(id)
            .and_then(|ws| ws.file_view(path).map(|(c, ..)| c.to_string()))
            .unwrap_or_default();
        let runs = editor::highlight(&content, path);
        let entity = cx.new(|cx| {
            let mut input = ChatInput::new_code(cx);
            input.set_text(content, cx);
            input.set_syntax(runs, cx);
            input
        });
        let sub = {
            let path = path.clone();
            cx.subscribe(
                &entity,
                move |this, input, event: &InputEvent, cx| match event {
                    InputEvent::Edited => {
                        let text = input.read(cx).text().to_string();
                        let runs = editor::highlight(&text, &path);
                        if let Some(ws) = this.supervisor.get_mut(id) {
                            ws.set_file_content(&path, text);
                        }
                        input.update(cx, |i, cx| i.set_syntax(runs, cx));
                        cx.notify();
                    }
                    InputEvent::Save => {
                        this.dispatch(DashAction::EditorSave(id, path.clone()), cx);
                    }
                    _ => {}
                },
            )
        };
        self.editor_inputs.insert(key, entity);
        self.chat_subs.push(sub);
    }

    /// The tree-op (rename/new) name input, created on demand.
    pub(crate) fn ensure_tree_op_input(&mut self, cx: &mut Context<Self>) {
        if self.tree_op_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("name\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
                this.dispatch(DashAction::TreeOpSubmit, cx);
            }
        });
        self.tree_op_input = Some(entity);
        self.chat_subs.push(sub);
    }

    /// Filter box for the sessions browser (created on first open).
    pub(crate) fn ensure_sessions_filter_input(&mut self, cx: &mut Context<Self>) {
        if self.sessions_filter_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("\u{1f50e} filter sessions\u{2026}", cx));
        // Edited just needs a repaint (the filter is read at render time).
        let sub = cx.subscribe(&entity, |_, _, _: &InputEvent, cx| cx.notify());
        self.sessions_filter_input = Some(entity);
        self.chat_subs.push(sub);
    }

    /// Drain-tick chat upkeep: consume turn-end thinking-collapse signals
    /// (egui's one-shot Cell) and sidecar requests to open the sessions
    /// browser (`/resume`). Bounded: scans only the active chat's tail.
    fn chat_upkeep(&mut self, cx: &mut Context<Self>) {
        let Screen::Chat(id) = self.screen else {
            return;
        };
        let mut wants_sessions = false;
        if let Some(ws) = self.supervisor.get_mut(id) {
            wants_sessions = ws.wants_sessions();
            const SCAN_TAIL: usize = 130; // render tail + slack
            let entries = ws.entries();
            let start = entries.len().saturating_sub(SCAN_TAIL);
            for (i, entry) in entries.iter().enumerate().skip(start) {
                if let crate::workspace::Entry::Thinking { collapse, .. } = entry
                    && collapse.get()
                {
                    collapse.set(false);
                    self.collapsed_thinking.insert((id.0, i));
                }
            }
        }
        if wants_sessions {
            self.dispatch(DashAction::OpenSessions(id), cx);
        }
    }

    /// Create the shared answer input on demand (ask Other / input prompts).
    pub(crate) fn ensure_answer_input(&mut self, cx: &mut Context<Self>) {
        if self.answer_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("Type your answer\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
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
        let sub = cx.subscribe(&entity, move |this, input, event: &InputEvent, cx| {
            this.on_chat_input_event(id, &input, event, cx);
        });
        self.chat_inputs.insert(id, entity);
        self.chat_subs.push(sub);
    }

    /// All composer-input events for one workspace, in one place.
    fn on_chat_input_event(
        &mut self,
        id: WorkspaceId,
        input: &Entity<ChatInput>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Edited => {
                let text = input.read(cx).text().to_string();
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.update_completions(&text);
                }
                self.palette_sel = 0;
                self.sync_palette_flag(id, cx);
                cx.notify();
            }
            InputEvent::Submitted => self.dispatch(DashAction::ChatSubmit(id), cx),
            InputEvent::HistoryPrev => {
                let draft = input.read(cx).text().to_string();
                let recalled = self
                    .supervisor
                    .get_mut(id)
                    .and_then(|ws| ws.history_prev(&draft));
                if let Some(text) = recalled {
                    // Suppress BEFORE set_text: the Edited event is delivered
                    // after this handler, and update_completions equality-
                    // debounces against last_query.
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        ws.suppress_completions_for(&text);
                    }
                    input.update(cx, |i, cx| i.set_text(text, cx));
                    self.sync_palette_flag(id, cx);
                }
            }
            InputEvent::HistoryNext => {
                let recalled = self.supervisor.get_mut(id).and_then(|ws| ws.history_next());
                if let Some(text) = recalled {
                    if let Some(ws) = self.supervisor.get_mut(id) {
                        ws.suppress_completions_for(&text);
                    }
                    input.update(cx, |i, cx| i.set_text(text, cx));
                    self.sync_palette_flag(id, cx);
                }
            }
            InputEvent::PaletteNav(delta) => {
                let n = self
                    .supervisor
                    .get(id)
                    .map(|ws| ws.completion_items().len().min(30))
                    .unwrap_or(0);
                if n > 0 {
                    let cur = self.palette_sel as i64 + *delta as i64;
                    self.palette_sel = cur.rem_euclid(n as i64) as usize;
                }
                cx.notify();
            }
            InputEvent::PaletteAccept => {
                self.dispatch(DashAction::ApplyCompletion(id, self.palette_sel), cx);
            }
            InputEvent::PaletteDismiss => {
                if let Some(ws) = self.supervisor.get_mut(id) {
                    ws.dismiss_completions();
                }
                self.sync_palette_flag(id, cx);
                cx.notify();
            }
            InputEvent::Save => {} // composer has nothing to save
            InputEvent::Image(png) => {
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                let img = std::sync::Arc::new(gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    png.clone(),
                ));
                self.pending_images
                    .entry(id)
                    .or_default()
                    .push(PendingImage { b64, img });
                let accent = self.tokens.accent;
                self.toast(
                    "Image attached \u{2014} sends with your next message".into(),
                    accent,
                );
                cx.notify();
            }
        }
    }

    /// Mirror a workspace's palette visibility onto its input entity (the
    /// input routes nav keys to the palette while this is set).
    fn sync_palette_flag(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        let open = self
            .supervisor
            .get(id)
            .map(|ws| ws.completions_open())
            .unwrap_or(false);
        if let Some(input) = self.chat_inputs.get(&id) {
            input.update(cx, |i, _| i.palette_open = open);
        }
    }

    /// Keep the active chat's palette flag fresh (sidecar completion replies
    /// arrive via the drain loop, not via input edits).
    fn sync_active_palette(&mut self, cx: &mut Context<Self>) {
        if let Screen::Chat(id) = self.screen {
            self.sync_palette_flag(id, cx);
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
                    root.save_session_if_changed();
                    root.sync_active_palette(cx);
                    root.chat_upkeep(cx);
                    root.pump_den();
                    root.maybe_probe_den(cx);
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

    /// Read-modify-write the shared session.json: UI prefs + the open-
    /// workspace list, preserving egui-only fields (theme, dock layout).
    /// A user flipping shells via the feature flag must never lose state.
    pub(crate) fn save_prefs(&mut self) {
        if self.session_no_save {
            return;
        }
        let mut s = session::load();
        s.dashboard_view = self.dash_mode;
        s.reduce_motion = self.reduce_motion;
        s.composer_style = self.composer_style;
        s.workspaces = self
            .supervisor
            .iter()
            .map(|w| session::WorkspaceEntry {
                path: w.root.to_string_lossy().into_owned(),
                agent: (!w.agent.is_empty()).then(|| w.agent.clone()),
                model: (!w.model.is_empty()).then(|| w.model.clone()),
                autosave: (!w.autosave.is_empty()).then(|| w.autosave.clone()),
            })
            .collect();
        session::save(&s);
    }

    /// Change-gated session save, run from the drain loop: persists when a
    /// workspace's path/agent/model/autosave set changes (sidecar announces
    /// agent/model after ready, so launch-time entries fill in shortly).
    fn save_session_if_changed(&mut self) {
        if self.session_no_save {
            return;
        }
        let sig: String = format!(
            "{:?}",
            self.supervisor
                .iter()
                .map(|w| (&w.root, &w.agent, &w.model, &w.autosave))
                .collect::<Vec<_>>()
        );
        if sig != self.session_sig {
            self.session_sig = sig;
            self.save_prefs(); // does NOT touch the sig — one bookkeeper
        }
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
        let den_part = self.den.as_ref().map(|d| {
            format!(
                "den[{} alive={} members={} roster={} feed={} tasks={} plans={}]",
                d.room,
                d.alive,
                d.state.members.len(),
                d.state.roster.len(),
                d.state.feed.len(),
                d.state.tasks.len(),
                d.state.plans.len()
            )
        });
        if self.supervisor.is_empty() {
            return den_part.unwrap_or_else(|| "no workspaces".to_string());
        }
        let mut line = self
            .supervisor
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
            .join(" | ");
        if let Some(d) = den_part {
            line.push_str(" | ");
            line.push_str(&d);
        }
        line
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
                    &if let Some(den_conn) = &self.den {
                        format!(
                            "\u{1f43e} {} \u{b7} {}",
                            crate::pack::DEN_LABEL,
                            den_conn.room
                        )
                    } else {
                        format!("\u{1f43e} Join {}", crate::pack::DEN_LABEL)
                    },
                )
                .id("den-toolbar")
                .when(self.den.is_some(), |d| {
                    d.border_color(alpha(self.tokens.run, 0.6))
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dispatch(DashAction::Den(den::DenAction::Show), cx)
                })),
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

        // Presence heuristic input: is the window focused right now?
        self.window_active = window.is_window_active();

        // One-shot: focus the composer when a chat was just opened.
        if let Some(id) = self.pending_focus.take()
            && let Some(input) = self.chat_inputs.get(&id)
        {
            window.focus(&input.read(cx).focus_handle(cx));
        }

        // A closed workspace can leave `screen` dangling — fall back to dash.
        if let Screen::Chat(id) = self.screen
            && self.supervisor.get(id).is_none()
        {
            self.screen = Screen::Dashboard;
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
        let active_chat = match self.screen {
            Screen::Chat(id) => Some(id),
            _ => None,
        };
        let strip = chat::tab_strip(
            &t,
            tabs,
            active_chat,
            self.den.as_ref().map(|d| (d.room.clone(), d.alive)),
            self.screen == Screen::Den,
            &entity,
        );

        let body: gpui::AnyElement = match self.screen {
            Screen::Den => self.den_body(cx),
            Screen::Chat(id) => {
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
                    images: self
                        .pending_images
                        .get(&id)
                        .map(|v| {
                            v.iter()
                                .enumerate()
                                .map(|(i, p)| (i, p.img.clone()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    palette_sel: self.palette_sel,
                    steer_queue: self.chat_steer_queue,
                    editor_input: match ws.editor_tabs().get(ws.editor_active_ix()) {
                        Some(crate::workspace::EditorItem::File(p)) => {
                            self.editor_inputs.get(&(id.0, p.clone()))
                        }
                        _ => None,
                    },
                    editor_close_confirm: self
                        .editor_close_confirm
                        .filter(|(cid, _)| *cid == id)
                        .map(|(_, ix)| ix),
                    markers: ws.tree_markers(),
                    tree_menu: match &self.chat_pop {
                        Some(ChatPop::TreeMenu(tid, path, is_dir)) if *tid == id => {
                            Some((path.clone(), *is_dir))
                        }
                        _ => None,
                    },
                    tree_op_input: self.tree_op_input.as_ref(),
                    tree_op_armed: self.tree_op.is_some(),
                    tree_delete_pending: self
                        .tree_delete_confirm
                        .as_ref()
                        .filter(|(cid, ..)| *cid == id)
                        .map(|(_, p, _)| p.clone()),
                    logs_open: self.logs_open.contains(&id),
                    collapsed_thinking: &self.collapsed_thinking,
                    sessions: (self.sessions_open == Some(id)).then(|| {
                        chat::sessions::SessionsArgs {
                            t,
                            ws,
                            root: entity.clone(),
                            filter_input: self.sessions_filter_input.as_ref(),
                            filter: self
                                .sessions_filter_input
                                .as_ref()
                                .map(|i| i.read(cx).text().to_string())
                                .unwrap_or_default(),
                            selected: self.session_selected.as_ref(),
                            puppy: puppy.clone(),
                        }
                    }),
                })
            }
            Screen::Dashboard => self.dashboard_body(cx),
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
                            .child(dashboard::attention_banner(
                                &t,
                                &waiting,
                                &entity,
                                self.reduce_motion,
                            ))
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
