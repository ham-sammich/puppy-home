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

pub mod about;
pub mod actions;
pub mod assets;
pub mod avatars;
pub mod browser_ui;
pub mod chat;
pub mod dashboard;
pub mod den;
pub mod editor;
pub mod gitpanel;
pub mod input;
pub mod managers;
pub mod managers_agents;
pub mod managers_agents_wizard;
pub mod managers_config;
pub mod managers_mcp;
pub mod managers_models;
pub mod managers_skills;
pub mod managers_ui;
pub mod markdown;
pub mod perf_ui;
pub mod remote;
pub mod remote_ui;
pub mod terminal;
pub mod theme_editor_ui;
pub mod theme_ui;
pub mod tokens;
pub mod waker;
pub mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    AnyElement, App, Application, Bounds, Context, Entity, FocusHandle, Focusable as _, FontWeight,
    IntoElement, ParentElement as _, Rgba, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};

use crate::browser::{BrowserId, BrowserManager};
use crate::session::{self, ComposerStyle, DashboardViewMode, Theme};
use crate::supervisor::Supervisor;
use crate::theme::{TerminalTheme, ThemePalette};
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
    /// The browser-plugin host surface (egui's dockable Browser tab).
    Browser,
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
            // P5: restore the last window size/position (with a sanity floor
            // so a corrupt/tiny saved rect can't open an unusable window).
            let saved = session::load();
            let bounds = match saved.window_rect {
                Some((x, y, w, h)) if w >= 480. && h >= 360. => Bounds {
                    origin: point(px(x), px(y)),
                    size: size(px(w), px(h)),
                },
                _ => Bounds::centered(None, size(px(1180.), px(760.)), cx),
            };
            let window_bounds = if saved.window_maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            };
            let options = WindowOptions {
                window_bounds: Some(window_bounds),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Doghouse")),
                    ..Default::default()
                }),
                ..Default::default()
            };
            input::bind_keys(cx);
            cx.open_window(options, |window, cx| {
                let view = cx.new(RootView::new);
                // Quit-confirm (#4): if a puppy is mid-turn, intercept the
                // window close, arm the confirm overlay, and veto the close
                // until the user decides.
                let view_close = view.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    view_close.update(cx, |root, cx| root.allow_close(cx))
                });
                view
            })
            .expect("failed to open the main window");
            cx.activate(true);
        });
}

/// Shared slot carrying the Browser screen's embed-region bounds
/// (window coords, logical px) from the layout canvas to render upkeep.
pub(crate) type EmbedSlot = std::sync::Arc<std::sync::Mutex<Option<(f32, f32, f32, f32)>>>;

pub struct RootView {
    supervisor: Supervisor,
    tokens: Tokens,
    /// Most recent open/spawn error, shown inline.
    last_error: Option<String>,
    /// `PUPPY_GPUI_PROMPT`: one-shot probe prompt (Task 2.1 instrumentation).
    probe_prompt: Option<String>,
    /// Log remote connect outcomes to stderr (set by the remote E2E probe).
    pub(crate) probe_remote_log: bool,
    /// Armed transcript dump: (workspace, sent-at, wait-secs).
    probe_prompt_dump: Option<(WorkspaceId, Instant, u64)>,
    /// Browser-embed region bounds (window coords, logical px), recorded
    /// by the Browser screen's canvas each layout — read at render start
    /// (one-frame lag, same pattern as the terminal resize slot).
    browser_embed_slot: EmbedSlot,
    /// Cached native view pointer (macOS NSView from raw-window-handle);
    /// lets the drain loop hide the overlay on minimize without a Window.
    browser_ns_view: Option<usize>,
    /// Whether the browser wake ticker (minimize-hide while idle) runs.
    browser_ticker: bool,
    /// Browser-cycle probe state (stage index, t0).
    browser_cycle_stage: u8,
    browser_cycle_at: Option<Instant>,
    // -- dashboard state --
    dash_mode: DashboardViewMode,
    reduce_motion: bool,
    toasts: Vec<Toast>,
    /// About / version panel state (QW1).
    pub(crate) about: about::AboutState,
    /// Avatar picker panel state + chosen emoji (QW8; empty = defaults).
    pub(crate) avatar_ui: avatars::AvatarUi,
    pub(crate) user_avatar: String,
    pub(crate) puppy_avatar: String,
    pub(crate) avatar_input: Option<Entity<ChatInput>>,
    card_input: Option<CardInput>,
    model_popover: Option<WorkspaceId>,
    input_focus: FocusHandle,
    // -- chat state --
    screen: Screen,
    composer_style: ComposerStyle,
    chat_inputs: HashMap<WorkspaceId, Entity<ChatInput>>,
    /// Per-workspace transcript scroll handle (so each chat keeps its own
    /// scroll position) and the entry count we last pinned at — together they
    /// keep the view glued to the newest turn while still letting you scroll
    /// up to read history (see the render-time pin logic).
    chat_scroll: HashMap<WorkspaceId, gpui::ScrollHandle>,
    chat_seen_len: HashMap<WorkspaceId, usize>,
    /// Scroll handle for the agent-creator modal's transcript preview.
    agent_creator_scroll: gpui::ScrollHandle,
    chat_subs: Vec<gpui::Subscription>,
    chat_pop: Option<ChatPop>,
    /// Transcript entries with an opened collapsible body (diff / thinking).
    expanded_entries: HashSet<(u64, usize)>,
    /// Workspaces rendering the full transcript ("Show older" clicked).
    show_all_chat: HashSet<WorkspaceId>,
    /// Workspaces with the explorer hidden (default = shown).
    tree_closed: HashSet<WorkspaceId>,
    /// Explorer hidden-entry policy, app-wide (F4; default Show).
    hidden_mode: crate::session::HiddenMode,
    expanded_dirs: HashSet<(u64, PathBuf)>,
    /// Focus the chat input on the next render (set when a chat opens).
    pending_focus: Option<WorkspaceId>,
    /// Focus the tree-op name input on the next render (set when a
    /// rename/new-file/new-folder op is armed, so the caret shows up).
    pending_tree_focus: bool,
    /// `PUPPY_GPUI_SCREEN=chat`: auto-open the first ready workspace's chat
    /// (probe instrumentation, like PUPPY_GPUI_PROMPT).
    probe_chat_screen: bool,
    /// Shared slot carrying the Browser screen's embed-region bounds
    /// (window coords, logical px) from the layout canvas to render upkeep.
    /// Shared single-line answer input (ask Other rows + input prompts).
    answer_input: Option<Entity<ChatInput>>,
    /// Which (workspace, ask-question) the answer input currently feeds.
    other_target: Option<(WorkspaceId, usize)>,
    // -- den state --
    /// Shared waker (PackClient connections need it).
    waker: std::sync::Arc<dyn UiWaker>,
    den: Option<DenConn>,
    /// Set while WE run the relay (QW6) — killed on stop/leave/app exit.
    den_host: Option<den::host::DenHost>,
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
    /// Member count at our last roster broadcast; when it changes we force a
    /// re-broadcast so newly-joined members see our agents (the change-gate
    /// would otherwise suppress it).
    den_roster_members: usize,
    /// Presence heuristic inputs (unfocused OR >5min since interaction).
    window_active: bool,
    /// Latest window placement, captured each render for persistence (P5):
    /// `(x, y, w, h)` logical px of the windowed/restore rect, + maximized.
    win_rect: Option<(f32, f32, f32, f32)>,
    win_maximized: bool,
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
    /// Editable path bar for the @File picker — lets you jump to any folder
    /// or drive instead of being trapped under the workspace root (F6).
    pub(crate) picker_path_input: Option<Entity<ChatInput>>,
    pub(crate) tree_delete_confirm: Option<(WorkspaceId, PathBuf, bool)>,
    /// Dashboard card awaiting a close confirmation (busy puppy only).
    pub(crate) card_close_confirm: Option<WorkspaceId>,
    /// A puppy is mid-turn and the user tried to quit — show the confirm
    /// overlay and block the close until they decide (#4).
    pub(crate) quit_confirm: bool,
    /// User confirmed "quit anyway" — the next close request sails through.
    pub(crate) quit_armed: bool,
    /// Window-space anchor for the floating tree context menu (set on
    /// right-click; read only while `chat_pop` is a `TreeMenu`).
    pub(crate) tree_menu_pos: Option<gpui::Point<gpui::Pixels>>,
    // -- git surface state --
    /// Commit-message inputs, one per workspace (height is a CONSTANT in
    /// gitpanel.rs — never content-derived; the 31a6dcb principle).
    pub(crate) commit_inputs: HashMap<WorkspaceId, Entity<ChatInput>>,
    /// Workspaces showing the flat history list (graph is the default).
    pub(crate) git_list_mode: HashSet<WorkspaceId>,
    /// Right-clicked graph row: (hash, short, refs).
    pub(crate) graph_menu: Option<(String, String, Vec<String>)>,
    pub(crate) branch_input: Option<Entity<ChatInput>>,
    pub(crate) branch_target: Option<(WorkspaceId, String)>,
    pub(crate) creds_user_input: Option<Entity<ChatInput>>,
    pub(crate) creds_pass_input: Option<Entity<ChatInput>>,
    // -- terminal state --
    term_focus: FocusHandle,
    term_colors: terminal::TermColors,
    term_resize: terminal::ResizeSlot,
    term_probe_stage: u8,
    term_probe_at: Instant,
    // -- manager state --
    pub(crate) manager_open: Option<managers::MgrKind>,
    pub(crate) mgr_inputs: Vec<Entity<ChatInput>>,
    pub(crate) mgr_paste_input: Option<Entity<ChatInput>>,
    pub(crate) mgr_selected: Option<String>,
    /// Which (workspace, catalog generation) the open manager last saw.
    pub(crate) mgr_seen: Option<(WorkspaceId, u64)>,
    pub(crate) mgr_last_request: Option<Instant>,
    /// Optimistic toggle overrides (name -> desired), cleared on fresh data.
    pub(crate) mgr_pending: HashMap<String, bool>,
    /// Models manager: extra_models.json editor open (QW4).
    pub(crate) models_editor: bool,
    /// Config manager: parsed puppy.cfg + the row in edit mode (QW5).
    pub(crate) cfg_entries: Vec<(String, String)>,
    pub(crate) cfg_edit_key: Option<String>,
    pub(crate) mcp_wizard: Option<crate::views::mcp_wizard::Wizard>,
    pub(crate) skills_wizard: Option<crate::views::skills_wizard::Wizard>,
    pub(crate) agent_wizard: Option<crate::views::agent_wizard::Wizard>,
    pub(crate) agent_delete_confirm: Option<String>,
    /// Agent wizard: the model dropdown is open.
    pub(crate) agent_model_menu: bool,
    /// The live Agent Creator chat session (an ephemeral workspace), shown
    /// in its own modal over the agents manager. `None` = not creating (F8).
    pub(crate) agent_creator_session: Option<WorkspaceId>,
    // -- remote connect state --
    pub(crate) remote: Option<remote::RemoteState>,
    pub(crate) remote_pending: Option<remote::RemotePending>,
    /// [0] = SSH target, [1] = remote path.
    pub(crate) remote_inputs: Vec<Entity<ChatInput>>,
    /// In-flight "puppush" (auth + models to a remote host); one at a time.
    pub(crate) creds_pending: Option<remote::CredsPush>,
    /// Armed two-step confirm for a workspace-toolbar creds push.
    pub(crate) creds_confirm: Option<WorkspaceId>,
    // -- theme state --
    pub(crate) theme: Theme,
    /// The saved custom-theme library (themes.json).
    pub(crate) themes: Vec<ThemePalette>,
    /// The editor's working palette buffer.
    pub(crate) theme_palette: ThemePalette,
    /// The editor's working terminal-palette buffer.
    pub(crate) terminal_theme: TerminalTheme,
    pub(crate) theme_picker_open: bool,
    pub(crate) theme_editor_open: bool,
    pub(crate) theme_inputs: Vec<Entity<ChatInput>>,
    // -- browser-plugin host state --
    pub(crate) browser: BrowserManager,
    /// The single GPUI browser surface's tab (egui docks N; we host one).
    pub(crate) browser_tab: Option<BrowserId>,
    pub(crate) browser_url_input: Option<Entity<ChatInput>>,
    /// Dashboard plugins-section expanded? (egui default_open = true)
    pub(crate) plugins_open: bool,
    /// Dev-server URLs detected in the active workspace's terminal/transcript
    /// (the chip row). Recomputed on a throttle by `dev_url_upkeep`.
    pub(crate) detected_dev_urls: Vec<String>,
    /// Dev-server URLs we've already auto-opened — so each one auto-opens at
    /// most once (the rest stay one-click chips). (#6 browser auto-open.)
    pub(crate) opened_dev_urls: std::collections::HashSet<String>,
    /// Throttle clock for the dev-URL scan.
    pub(crate) last_dev_scan: Option<Instant>,
    // -- perf HUD --
    pub(crate) perf: perf_ui::GpuiPerf,
    // -- den pack-sync (activity broadcast + Tier-2 breadcrumb) --
    pub(crate) pack_activity_at: Option<Instant>,
    pub(crate) pack_activity_last: String,
    pub(crate) pack_breadcrumb_sig: String,
    pub(crate) pack_breadcrumb_at: Option<Instant>,
    pub(crate) pack_breadcrumb_written: bool,
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

        // Theme: restore the saved selection and publish its tokens before
        // any input entity exists (they read Tokens::current at creation).
        let themes = crate::theme::load_themes();
        let theme = saved.theme.clone();
        let theme_palette = crate::theme::palette_for(&theme, &themes);
        let resolved_tokens = Tokens::from_palette(&theme_palette);
        Tokens::set_current(resolved_tokens);

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
            tokens: resolved_tokens,
            last_error,
            probe_prompt: std::env::var("PUPPY_GPUI_PROMPT").ok(),
            probe_remote_log: false,
            probe_prompt_dump: None,
            browser_embed_slot: EmbedSlot::default(),
            browser_ns_view: None,
            browser_ticker: false,
            browser_cycle_stage: 0,
            browser_cycle_at: None,
            dash_mode: saved.dashboard_view,
            reduce_motion: saved.reduce_motion,
            avatar_ui: avatars::AvatarUi::default(),
            user_avatar: saved.user_avatar.clone(),
            puppy_avatar: saved.puppy_avatar.clone(),
            avatar_input: None,
            toasts: Vec::new(),
            about: about::AboutState::default(),
            card_input: None,
            model_popover: None,
            input_focus: cx.focus_handle(),
            screen: Screen::Dashboard,
            composer_style: saved.composer_style,
            chat_inputs: HashMap::new(),
            chat_scroll: HashMap::new(),
            chat_seen_len: HashMap::new(),
            agent_creator_scroll: gpui::ScrollHandle::new(),
            chat_subs: Vec::new(),
            chat_pop: None,
            expanded_entries: HashSet::new(),
            show_all_chat: HashSet::new(),
            tree_closed: HashSet::new(),
            hidden_mode: saved.hidden_mode,
            expanded_dirs: HashSet::new(),
            pending_focus: None,
            pending_tree_focus: false,
            probe_chat_screen: std::env::var("PUPPY_GPUI_SCREEN").as_deref() == Ok("chat"),
            answer_input: None,
            other_target: None,
            waker,
            den: None,
            den_host: None,
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
            den_roster_members: 0,
            window_active: true,
            win_rect: None,
            win_maximized: false,
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
            picker_path_input: None,
            tree_delete_confirm: None,
            card_close_confirm: None,
            quit_confirm: false,
            quit_armed: false,
            tree_menu_pos: None,
            commit_inputs: HashMap::new(),
            git_list_mode: HashSet::new(),
            graph_menu: None,
            branch_input: None,
            branch_target: None,
            creds_user_input: None,
            creds_pass_input: None,
            term_focus: cx.focus_handle(),
            term_colors: terminal::TermColors::load(),
            term_resize: std::sync::Arc::new(std::sync::Mutex::new(None)),
            term_probe_stage: 0,
            term_probe_at: Instant::now(),
            manager_open: None,
            mgr_inputs: Vec::new(),
            mgr_paste_input: None,
            mgr_selected: None,
            mgr_seen: None,
            mgr_last_request: None,
            mgr_pending: HashMap::new(),
            models_editor: false,
            cfg_entries: Vec::new(),
            cfg_edit_key: None,
            mcp_wizard: None,
            skills_wizard: None,
            agent_wizard: None,
            agent_model_menu: false,
            agent_creator_session: None,
            agent_delete_confirm: None,
            remote: None,
            remote_pending: None,
            creds_pending: None,
            creds_confirm: None,
            remote_inputs: Vec::new(),
            theme,
            themes,
            theme_palette,
            terminal_theme: crate::theme::load_terminal(),
            theme_picker_open: false,
            theme_editor_open: false,
            theme_inputs: Vec::new(),
            browser: BrowserManager::discover(),
            browser_tab: None,
            detected_dev_urls: Vec::new(),
            opened_dev_urls: std::collections::HashSet::new(),
            last_dev_scan: None,
            browser_url_input: None,
            plugins_open: true,
            perf: perf_ui::GpuiPerf::default(),
            pack_activity_at: None,
            pack_activity_last: String::new(),
            pack_breadcrumb_sig: String::new(),
            pack_breadcrumb_at: None,
            pack_breadcrumb_written: false,
        }
    }

    /// Apply the grid size the terminal canvas measured last paint (an
    /// element can't mutate entities mid-paint; one-frame lag, like egui's
    /// same-frame resize minus one tick).
    fn apply_terminal_resize(&mut self) {
        let Some((id, rows, cols)) = self.term_resize.lock().ok().and_then(|mut s| s.take()) else {
            return;
        };
        if let Some(term) = self.supervisor.get_mut(id).and_then(|ws| ws.terminal_mut()) {
            term.resize_to(rows, cols);
        }
    }

    /// Commit-message input for a workspace (multiline soft-wrap composer).
    pub(crate) fn ensure_commit_input(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if self.commit_inputs.contains_key(&id) {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("Commit message\u{2026}", cx));
        // No Submitted wiring: Enter newlines would be nice, but the composer
        // semantics send on Enter — commits go through the button instead.
        let sub = cx.subscribe(&entity, |_, _, _: &InputEvent, cx| cx.notify());
        self.commit_inputs.insert(id, entity);
        self.chat_subs.push(sub);
    }

    pub(crate) fn ensure_branch_input(&mut self, cx: &mut Context<Self>) {
        if self.branch_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("new branch name\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
                this.dispatch(DashAction::GraphBranchSubmit, cx);
            }
        });
        self.branch_input = Some(entity);
        self.chat_subs.push(sub);
    }

    /// Username/password inputs for the git-credentials modal, created when
    /// a prompt first appears (drain-driven, like the answer input).
    fn ensure_creds_inputs_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.creds_user_input.is_some() {
            return;
        }
        let needed = self
            .supervisor
            .iter()
            .any(|w| w.git_creds_prompt().is_some());
        if !needed {
            return;
        }
        let user = cx.new(|cx| ChatInput::new("username", cx));
        let pass = cx.new(|cx| ChatInput::new("password / token", cx));
        let s1 = cx.subscribe(&user, |_, _, _: &InputEvent, cx| cx.notify());
        let s2 = cx.subscribe(&pass, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted)
                && let Screen::Chat(id) = this.screen
            {
                this.dispatch(DashAction::CredsSubmit(id), cx);
            }
        });
        self.creds_user_input = Some(user);
        self.creds_pass_input = Some(pass);
        self.chat_subs.extend([s1, s2]);
    }

    /// Create (once) the code-editor input for an open file: seeded from the
    /// buffer, syntax-highlighted on load and on every edit (200 KB cap),
    /// Cmd/Ctrl+S routed to the save action.
    pub(crate) fn ensure_editor_input(
        &mut self,
        id: WorkspaceId,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        let key = (id.0, path.to_path_buf());
        if self.editor_inputs.contains_key(&key) {
            return;
        }
        let content = self
            .supervisor
            .get(id)
            .and_then(|ws| ws.file_view(path).map(|(c, ..)| c.to_string()))
            .unwrap_or_default();
        let runs = editor::highlight(&content, path, self.tokens.dark);
        let entity = cx.new(|cx| {
            let mut input = ChatInput::new_code(cx);
            input.set_text(content, cx);
            input.set_syntax(runs, cx);
            input
        });
        let sub = {
            let path = path.to_path_buf();
            cx.subscribe(
                &entity,
                move |this, input, event: &InputEvent, cx| match event {
                    InputEvent::Edited => {
                        // Update the buffer immediately (cheap), but DEBOUNCE the
                        // full-file syntect re-highlight: running it on every
                        // keystroke is what made the editor laggy (#1a). The
                        // typed char paints from the input's own re-render; the
                        // coloring catches up ~120ms after you pause.
                        let text = input.read(cx).text().to_string();
                        if let Some(ws) = this.supervisor.get_mut(id) {
                            ws.set_file_content(&path, text.clone());
                        }
                        cx.notify();
                        let input = input.clone();
                        let path = path.clone();
                        let dark = this.tokens.dark;
                        cx.spawn(async move |_this, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(120))
                                .await;
                            // Only the keystroke whose snapshot still matches the
                            // live text reshapes — intermediate ones bail.
                            let _ = input.update(cx, |i, cx| {
                                if i.text() == text {
                                    let runs = editor::highlight(&text, &path, dark);
                                    i.set_syntax(runs, cx);
                                }
                            });
                        })
                        .detach();
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

    /// The @File picker's path bar (created on first open); Enter jumps to
    /// the typed folder/drive (F6).
    pub(crate) fn ensure_picker_path_input(&mut self, cx: &mut Context<Self>) {
        if self.picker_path_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("path or drive (e.g. D:\\)\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
                this.dispatch(DashAction::PickerGoPath, cx);
            }
        });
        self.picker_path_input = Some(entity);
        self.chat_subs.push(sub);
    }

    /// Reflect the picker's current directory into its path bar so it reads
    /// like a breadcrumb you can edit (F6).
    pub(crate) fn seed_picker_path(&mut self, dir: &std::path::Path, cx: &mut Context<Self>) {
        self.ensure_picker_path_input(cx);
        if let Some(input) = self.picker_path_input.clone() {
            let text = dir.to_string_lossy().into_owned();
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
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

    /// Background `git status` for the visible workspace's tree markers /
    /// Changes list (the egui poll_git, drain-driven; 4s self-gate inside).
    fn poll_active_git(&mut self) {
        if let Screen::Chat(id) = self.screen
            && let Some(ws) = self.supervisor.get_mut(id)
        {
            let focused = self.window_active;
            ws.poll_git_status(focused, &self.waker);
        }
    }

    /// Drain-tick chat upkeep: consume turn-end thinking-collapse signals
    /// (egui's one-shot Cell) and sidecar requests to open the sessions
    /// browser (`/resume`). Bounded: scans only the active chat's tail.
    fn chat_upkeep(&mut self, cx: &mut Context<Self>) {
        let Screen::Chat(id) = self.screen else {
            return;
        };
        let mut wants_sessions = false;
        let mut wants_agent = false;
        let mut wants_model = false;
        if let Some(ws) = self.supervisor.get_mut(id) {
            wants_sessions = ws.wants_sessions();
            wants_agent = ws.wants_agent_picker();
            wants_model = ws.wants_model_picker();
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
        // Bare /agent and /model typed in chat behave like the CLI: the
        // switcher opens (the composer's own popovers, B13.4).
        if wants_agent {
            self.chat_pop = Some(ChatPop::Agent(id));
            cx.notify();
        }
        if wants_model {
            self.chat_pop = Some(ChatPop::Model(id));
            cx.notify();
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
        let needed = self.supervisor.iter_visible().any(|w| {
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

    /// Probe: `PUPPY_GPUI_TERM=1` — once the chat is open, toggle the
    /// terminal, run `ls`, then dump the vt100 grid text to stderr.
    fn maybe_probe_terminal(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("PUPPY_GPUI_TERM").is_none() {
            return;
        }
        let Screen::Chat(id) = self.screen else {
            return;
        };
        match self.term_probe_stage {
            0 => {
                self.dispatch(DashAction::TermToggle(id), cx);
                self.term_probe_stage = 1;
                self.term_probe_at = Instant::now();
            }
            1 => {
                // Give the shell a moment to print its prompt.
                if self.term_probe_at.elapsed() > Duration::from_secs(3) {
                    self.dispatch(DashAction::TermInput(id, b"ls\r".to_vec()), cx);
                    self.term_probe_stage = 2;
                    self.term_probe_at = Instant::now();
                }
            }
            2 => {
                if self.term_probe_at.elapsed() > Duration::from_secs(3)
                    && let Some(term) = self.supervisor.get(id).and_then(|w| w.terminal_ref())
                {
                    eprintln!("[probe] terminal grid:\n{}", term.screen_text());
                    self.term_probe_stage = 3;
                }
            }
            _ => {}
        }
    }

    /// Probe: `PUPPY_GPUI_MGR=mcp|skills|agents` opens a manager overlay
    /// once a sidecar is ready (render-survival validation).
    fn maybe_probe_manager(&mut self, cx: &mut Context<Self>) {
        if self.manager_open.is_some() || self.first_ready_ws().is_none() {
            return;
        }
        let Ok(kind) = std::env::var("PUPPY_GPUI_MGR") else {
            return;
        };
        let kind = match kind.as_str() {
            "mcp" => managers::MgrKind::Mcp,
            "skills" => managers::MgrKind::Skills,
            "agents" => managers::MgrKind::Agents,
            _ => return,
        };
        unsafe { std::env::remove_var("PUPPY_GPUI_MGR") };
        self.dispatch(DashAction::Mgr(managers::MgrAction::Open(kind)), cx);
        eprintln!("[probe] opened manager overlay: {kind:?}");
    }

    /// Probe: `PUPPY_GPUI_THEME=dark|light|<custom name>` picks a theme at
    /// startup; `PUPPY_GPUI_REMOTE=1` opens the connect dialog
    /// (render-survival validation for the Phase E surfaces).
    fn maybe_probe_theme_remote(&mut self, cx: &mut Context<Self>) {
        if let Ok(spec) = std::env::var("PUPPY_GPUI_THEME") {
            unsafe { std::env::remove_var("PUPPY_GPUI_THEME") };
            let theme = match spec.as_str() {
                "dark" => Theme::Dark,
                "light" => Theme::Light,
                name => Theme::Custom(name.to_string()),
            };
            self.dispatch(DashAction::Theme(theme_ui::ThemeAction::Pick(theme)), cx);
            self.dispatch(DashAction::Theme(theme_ui::ThemeAction::EditorOpen), cx);
            eprintln!("[probe] picked theme {spec:?} + opened the editor");
        }
        if let Ok(spec) = std::env::var("PUPPY_GPUI_REMOTE") {
            unsafe { std::env::remove_var("PUPPY_GPUI_REMOTE") };
            self.dispatch(DashAction::Remote(remote::RemoteAction::Open), cx);
            eprintln!("[probe] opened the remote-connect dialog");
            // `PUPPY_GPUI_REMOTE=<target>:</path>` drives a full connect
            // (the headless remote-stack E2E); plain `1` just opens it.
            if spec != "1" {
                self.probe_remote_connect(&spec, cx);
            }
        }
        if let Some(v) = std::env::var_os("PUPPY_GPUI_BROWSER") {
            unsafe { std::env::remove_var("PUPPY_GPUI_BROWSER") };
            self.dispatch(DashAction::Browser(browser_ui::BrowserAction::Open), cx);
            eprintln!("[probe] opened the browser surface");
            // `PUPPY_GPUI_BROWSER=launch[:<url>]` also launches the plugin
            // (E2E; the optional url is the UA-canary hook).
            let v = v.to_string_lossy().to_string();
            if (v == "launch" || v.starts_with("launch:")) && self.browser.is_available() {
                if let Some(url) = v.strip_prefix("launch:")
                    && let Some(input) = &self.browser_url_input
                {
                    let url = url.to_string();
                    input.update(cx, |i, cx| i.set_text(&url, cx));
                }
                self.dispatch(DashAction::Browser(browser_ui::BrowserAction::Launch), cx);
                eprintln!("[probe] launched the browser plugin");
            }
        }
        if std::env::var_os("PUPPY_GPUI_PERF").is_some() {
            unsafe { std::env::remove_var("PUPPY_GPUI_PERF") };
            self.dispatch(DashAction::PerfToggle, cx);
            eprintln!("[probe] toggled the perf HUD");
        }
    }

    /// Probe: jump to the first ready workspace's chat once, if asked to.
    fn maybe_probe_chat_screen(&mut self, cx: &mut Context<Self>) {
        if !self.probe_chat_screen {
            return;
        }
        let Some(id) = self
            .supervisor
            .iter_visible()
            .find(|w| w.is_ready())
            .map(|w| w.id)
        else {
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
        // The workspace's OWN puppy (a remote workspace talks to the remote
        // host's puppy). May still read "Puppy" if the sidecar hasn't
        // announced yet — the placeholder is fixed at creation; cosmetic.
        let puppy = self
            .supervisor
            .get(id)
            .map(ws_puppy)
            .unwrap_or_else(|| "Puppy".to_string());
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
                    root.about.drain();
                    root.save_session_if_changed();
                    root.sync_active_palette(cx);
                    root.chat_upkeep(cx);
                    root.poll_active_git();
                    root.ensure_creds_inputs_if_needed(cx);
                    root.pump_den();
                    root.maybe_probe_den(cx);
                    root.maybe_send_probe_prompt();
                    root.maybe_probe_chat_screen(cx);
                    root.maybe_probe_terminal(cx);
                    root.maybe_probe_manager(cx);
                    root.maybe_probe_theme_remote(cx);
                    root.mgr_upkeep();
                    // Browser overlay discipline outside the render path:
                    // hide it when the host window minimizes (renders stop,
                    // so render-upkeep can't do it; the wake ticker keeps
                    // this loop breathing while idle).
                    root.browser_minimize_check();
                    root.maybe_probe_browser_cycle(cx);
                    root.remote_upkeep(cx);
                    root.pack_sync_upkeep();
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
        s.hidden_mode = self.hidden_mode;
        s.composer_style = self.composer_style;
        s.user_avatar = self.user_avatar.clone();
        s.puppy_avatar = self.puppy_avatar.clone();
        s.theme = self.theme.clone();
        if let Some(r) = self.win_rect {
            s.window_rect = Some(r);
        }
        s.window_maximized = self.win_maximized;
        s.workspaces = self
            .supervisor
            .iter_visible()
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
        // Round the window rect so sub-pixel jitter during a drag/resize
        // doesn't thrash the save; a settled new size/pos still triggers one.
        let win = self
            .win_rect
            .map(|(x, y, w, h)| (x as i32, y as i32, w as i32, h as i32));
        let sig: String = format!(
            "{:?}|{:?}|{}",
            self.supervisor
                .iter_visible()
                .map(|w| (&w.root, &w.agent, &w.model, &w.autosave))
                .collect::<Vec<_>>(),
            win,
            self.win_maximized,
        );
        if sig != self.session_sig {
            self.session_sig = sig;
            self.save_prefs(); // does NOT touch the sig — one bookkeeper
        }
    }

    /// The HEADLINE puppy identity (title chip, dashboard lede, Den): the
    /// first LOCAL sidecar's announced name. Remote sidecars announce the
    /// remote host's puppy — that identity belongs to its workspace's own
    /// surfaces, never the app-global headline (B13.8).
    fn puppy_name(&self) -> String {
        headline_puppy(self.supervisor.iter_visible().map(|w| {
            // SSH-fallback sidecars run LOCALLY — their announcement IS the
            // local puppy, so they stay headline-eligible (B13.8 semantics).
            (w.puppy_name.as_str(), w.is_remote() && !w.remote_fallback())
        }))
        .to_string()
    }

    /// The Agent Creator modal (F8): a focused chat with code_puppy's
    /// built-in `agent-creator` agent, running on a HIDDEN ephemeral session.
    /// Floats above the agents manager; "Done" kills the session (no card).
    fn agent_creator_modal(&self, t: &Tokens, entity: &Entity<RootView>) -> Option<AnyElement> {
        let id = self.agent_creator_session?;
        let ws = self.supervisor.get(id)?;
        let input = self.chat_inputs.get(&id)?.clone();
        let puppy = ws_puppy(ws);

        let transcript = chat::transcript::transcript_panel(&chat::transcript::TranscriptArgs {
            t: *t,
            ws,
            root: entity.clone(),
            puppy: puppy.clone(),
            user_avatar: self.user_avatar().to_string(),
            puppy_avatar: self.puppy_avatar().to_string(),
            show_all: true,
            expanded: &self.expanded_entries,
            collapsed_thinking: &self.collapsed_thinking,
            reduce_motion: self.reduce_motion,
            scroll: self.agent_creator_scroll.clone(),
        });
        let dock = chat::composer::composer_dock(&chat::composer::ComposerArgs {
            t: *t,
            ws,
            root: entity.clone(),
            input,
            style: self.composer_style,
            pop: self.chat_pop.as_ref(),
            puppy,
            images: Vec::new(),
            palette_sel: self.palette_sel,
            steer_queue: self.chat_steer_queue,
            picker_path_input: self.picker_path_input.as_ref(),
        });

        let header =
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child("\u{1fa84} Agent Creator"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.dim)
                        .child("chat to build an agent \u{00b7} closing discards the session"),
                )
                .child(div().flex_1())
                .child(widgets::btn(t, "Done").id("agent-creator-done").on_click(
                    managers_ui::act(entity, managers::MgrAction::AgentCreatorClose),
                ));

        let panel = div()
            .occlude()
            .w(px(760.))
            .max_w_full()
            .h(px(620.))
            .max_h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(13.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            .child(header)
            .child(div().flex_1().min_h_0().child(transcript))
            .child(dock);

        Some(
            gpui::deferred(
                div()
                    .absolute()
                    .inset_0()
                    // Occlude so backdrop clicks don't leak to the agents
                    // manager beneath (its on_mouse_down_out would close it).
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(widgets::alpha(t.bg, 0.6))
                    .child(panel),
            )
            .with_priority(240)
            .into_any_element(),
        )
    }

    /// Fire the probe prompt once the first sidecar reports ready.
    fn maybe_send_probe_prompt(&mut self) {
        let Some(prompt) = &self.probe_prompt else {
            // After a dump-armed prompt: print the tail of the transcript
            // once the wait elapses (headless agent-behavior verification).
            if let Some((id, at, secs)) = self.probe_prompt_dump
                && at.elapsed() > Duration::from_secs(secs)
            {
                self.probe_prompt_dump = None;
                if let Some(ws) = self.supervisor.get(id) {
                    eprintln!("[probe] transcript tail of {}:", ws.name);
                    for e in ws.entries().iter().rev().take(12).collect::<Vec<_>>() {
                        use crate::workspace::Entry;
                        let (tag, text) = match e {
                            Entry::User(t) => ("user", t.as_str()),
                            Entry::Agent(t) => ("agent", t.as_str()),
                            Entry::Note(t) => ("note", t.as_str()),
                            Entry::Error(t) => ("error", t.as_str()),
                            Entry::Thinking { text, .. } => ("thinking", text.as_str()),
                            Entry::Message(_) => ("message", "<structured>"),
                        };
                        let text: String = text.chars().take(600).collect();
                        eprintln!("  [{tag}] {text}");
                    }
                }
            }
            return;
        };
        // `PUPPY_GPUI_PROMPT_REMOTE=1` restricts the target to a REMOTE
        // workspace (the fallback E2E: session-restored local workspaces
        // become ready first and would steal the prompt otherwise).
        let want_remote = std::env::var_os("PUPPY_GPUI_PROMPT_REMOTE").is_some();
        let Some(id) = self
            .supervisor
            .iter()
            .find(|w| w.is_ready() && (!want_remote || w.is_remote()))
            .map(|w| w.id)
        else {
            return;
        };
        let prompt = prompt.clone();
        if let Some(ws) = self.supervisor.get_mut(id) {
            eprintln!("[probe] sending prompt to {}: {prompt:?}", ws.name);
            ws.send_prompt_text(&prompt);
            self.probe_prompt = None;
            // `PUPPY_GPUI_PROMPT_DUMP=<secs>` arms the transcript dump.
            if let Some(secs) = std::env::var("PUPPY_GPUI_PROMPT_DUMP")
                .ok()
                .and_then(|s| s.parse().ok())
            {
                self.probe_prompt_dump = Some((id, Instant::now(), secs));
                // The drain loop is waker-driven: once the agent's turn
                // ends there are no events, so a timed dump would never
                // fire. Tick the waker until past the deadline (+grace).
                let waker = self.waker.clone();
                std::thread::spawn(move || {
                    for _ in 0..(secs + 30) / 2 {
                        std::thread::sleep(Duration::from_secs(2));
                        waker.wake();
                    }
                });
            }
        }
    }

    /// Per-render browser presentation upkeep (E8 redux #2 — real
    /// embedding). Default mode glues the plugin's borderless overlay to
    /// the Browser screen's viewport region: NSView (via raw-window-handle
    /// `RawWindowHandle::AppKit`) \u{2192} Cocoa converts the canvas-recorded
    /// element rect to global top-left physical px \u{2192} `embed` IPC, re-sent
    /// every render exactly like the egui per-frame pump (z-order
    /// re-assert); `request_animation_frame` keeps renders flowing while
    /// the screen is up. Floating mode (\u{2197}) is the d6f8017 float path.
    fn browser_embed_upkeep(&mut self, window: &mut Window) {
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let Some(id) = self.browser_tab else { return };
            if !self.browser.tab_running(id) {
                return;
            }
            if self.browser_ns_view.is_none()
                && let Ok(h) = window.window_handle()
                && let RawWindowHandle::AppKit(ak) = h.as_raw()
            {
                self.browser_ns_view = Some(ak.ns_view.as_ptr() as usize);
            }
            let Some(view) = self.browser_ns_view else {
                return;
            };
            let view = view as *mut std::ffi::c_void;
            use crate::browser::{EmbedMode, embed_mac};
            match self.browser.tab_mode(id) {
                EmbedMode::Floating => self.browser.float_tab(id),
                EmbedMode::Embedded => {
                    let on_browser = self.screen == Screen::Browser;
                    if on_browser && self.browser.tab_ready(id) && !embed_mac::is_miniaturized(view)
                    {
                        let rect = *self.browser_embed_slot.lock().unwrap();
                        if let (Some(rect), Some(parent)) = (rect, embed_mac::window_number(view))
                            && let Some(px_rect) = embed_mac::view_rect_to_screen_px(view, rect)
                        {
                            self.browser.embed_tab(id, px_rect, parent);
                        }
                        // RAF keeps the child NSWindow glued during window
                        // drags — but only while WE are the active window
                        // (drags imply focus; an unfocused app spinning at
                        // vsync is pure waste — G1 audit). The 1s drain
                        // notify self-heals the rare Cmd-drag-unfocused case.
                        if self.window_active {
                            window.request_animation_frame();
                        }
                    } else {
                        self.browser.hide_tab(id);
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            // Owned-overlay glue (G3): attach to the GPUI HWND as an owned
            // borderless popup once, then place at the canvas rect in client
            // px (converted to screen px inside embed_tab_win). SetParent/
            // WS_CHILD is a trap here — the GPUI window is a DComp surface
            // that composes over child HWNDs (probe: EX_NOREDIRECTIONBITMAP).
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let Some(id) = self.browser_tab else { return };
            if !self.browser.tab_running(id) {
                return;
            }
            if self.browser_ns_view.is_none()
                && let Ok(h) = window.window_handle()
                && let RawWindowHandle::Win32(w32) = h.as_raw()
            {
                self.browser_ns_view = Some(w32.hwnd.get() as usize);
            }
            let Some(parent) = self.browser_ns_view else {
                return;
            };
            use crate::browser::EmbedMode;
            match self.browser.tab_mode(id) {
                EmbedMode::Floating => self.browser.float_tab(id),
                EmbedMode::Embedded => {
                    if self.screen == Screen::Browser {
                        let rect = *self.browser_embed_slot.lock().unwrap();
                        let scale = window.scale_factor();
                        if let Some((x, y, w, h)) = rect {
                            let to = |v: f32| (v * scale).round() as i32;
                            self.browser.embed_tab_win(
                                id,
                                parent as i64,
                                (to(x), to(y), to(w), to(h)),
                            );
                        }
                        // Same focus gate as the macOS branch (G1 audit).
                        if self.window_active {
                            window.request_animation_frame();
                        }
                    } else {
                        self.browser.hide_tab(id);
                    }
                }
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = window;
            // Linux: no embedding; float once so the window is visible
            // (parity with the egui shell's separate-window behavior).
            if let Some(id) = self.browser_tab
                && self.browser.tab_running(id)
            {
                self.browser.float_tab(id);
            }
        }
    }

    /// Probe: `PUPPY_GPUI_BROWSER_CYCLE=1` — staged embed-discipline E2E
    /// (needs the browser launched): +6s switch to Dashboard (overlay must
    /// hide), +12s back to Browser (re-embed), +18s pop out (decorated
    /// float), +24s pop back in. External tooling snapshots window state
    /// between stages.
    fn maybe_probe_browser_cycle(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("PUPPY_GPUI_BROWSER_CYCLE").is_none() {
            return;
        }
        let Some(id) = self.browser_tab else { return };
        if !self.browser.tab_running(id) {
            return;
        }
        if self.browser_cycle_at.is_none() {
            self.browser_cycle_at = Some(Instant::now());
        }
        let elapsed = self.browser_cycle_at.unwrap().elapsed().as_secs();
        let stage = self.browser_cycle_stage;
        let actions: [(u64, &str); 5] = [
            (6, "dashboard"),
            (12, "browser"),
            (18, "popout"),
            (24, "popin"),
            (30, "close"),
        ];
        if let Some(&(at, what)) = actions.get(stage as usize)
            && elapsed >= at
        {
            self.browser_cycle_stage += 1;
            eprintln!("[probe] browser-cycle stage {stage}: {what}");
            match what {
                "dashboard" => self.screen = Screen::Dashboard,
                "browser" => self.screen = Screen::Browser,
                "popout" => {
                    self.dispatch(DashAction::Browser(browser_ui::BrowserAction::PopOut), cx)
                }
                "popin" => self.dispatch(DashAction::Browser(browser_ui::BrowserAction::PopIn), cx),
                "close" => self.dispatch(
                    DashAction::Browser(browser_ui::BrowserAction::CloseSurface),
                    cx,
                ),
                _ => {}
            }
            cx.notify();
        }
    }

    /// Drain-side companion: hide the overlay while minimized (no renders).
    fn browser_minimize_check(&mut self) {
        #[cfg(target_os = "macos")]
        if let (Some(id), Some(view)) = (self.browser_tab, self.browser_ns_view)
            && crate::browser::embed_mac::is_miniaturized(view as *mut std::ffi::c_void)
        {
            self.browser.hide_tab(id);
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
        if self.supervisor.visible_is_empty() {
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

    /// Your avatar emoji (transcript "you" rows). Empty pref = default.
    pub(crate) fn user_avatar(&self) -> &str {
        if self.user_avatar.is_empty() {
            avatars::USER_DEFAULT
        } else {
            &self.user_avatar
        }
    }

    /// Your puppy's avatar emoji. Empty pref = default.
    pub(crate) fn puppy_avatar(&self) -> &str {
        if self.puppy_avatar.is_empty() {
            avatars::PUPPY_DEFAULT
        } else {
            &self.puppy_avatar
        }
    }

    /// The installed code_puppy version, from the first sidecar that
    /// announced one (they all resolve the same uv cache locally).
    pub(crate) fn cp_version_label(&self) -> String {
        self.supervisor
            .iter()
            .map(|w| w.cp_version.as_str())
            .find(|v| !v.is_empty() && *v != "?")
            .unwrap_or("")
            .to_string()
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
            .flex_wrap()
            .items_center()
            .gap_2()
            // Brand first, then the two version chips to its right. Each
            // chip is prefixed so they can't be confused; both open the
            // About panel (which spells out the full labels). P3.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text)
                            .child("\u{1f3e0} Doghouse"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.weak)
                            .child(format!("Home for {}", self.puppy_name())),
                    ),
            )
            .child({
                let ver_chip = |id: &'static str, label: String, tip: &'static str| {
                    div()
                        .id(id)
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(t.card)
                        .border_1()
                        .border_color(t.line_soft)
                        .text_size(px(10.5))
                        .text_color(t.weak)
                        .cursor_pointer()
                        .tooltip(widgets::text_tip(tip.into()))
                        .child(label)
                };
                let cp = self.cp_version_label();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        ver_chip(
                            "tb-app-version",
                            format!("app v{}", crate::plugin::HOST_VERSION),
                            "Doghouse (this app) version — click for details",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dispatch(DashAction::About(about::AboutAction::Toggle), cx)
                        })),
                    )
                    .child(
                        ver_chip(
                            "tb-about",
                            format!(
                                "cp {}",
                                if cp.is_empty() {
                                    "v?".into()
                                } else {
                                    format!("v{cp}")
                                }
                            ),
                            "code_puppy (agent engine) version + updates",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dispatch(DashAction::About(about::AboutAction::Toggle), cx)
                        })),
                    )
            })
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
                    .id("tb-identity")
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .tooltip(widgets::text_tip(
                        "Change profile pictures (photo or emoji)".into(),
                    ))
                    .child(avatars::boxed(self.puppy_avatar(), 16., 4.))
                    .child(self.puppy_name())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(DashAction::Avatar(avatars::AvatarAction::Toggle), cx)
                    })),
            )
            .child(
                widgets::btn(t, "\u{2795} New Chat")
                    .id("tb-new-chat")
                    .tooltip(widgets::text_tip(
                        "Spawn a Code Puppy at your home directory".into(),
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(DashAction::OpenHome { to_chat: true }, cx)
                    })),
            )
            .child(
                widgets::btn(t, "\u{1f4c1} Open Folder\u{2026}")
                    .id("open-folder")
                    .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
            )
            .child(
                widgets::btn(t, "\u{1f517} Connect Remote\u{2026}")
                    .id("tb-remote")
                    .tooltip(widgets::text_tip(
                        "Run a Code Puppy on another host over SSH".into(),
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(DashAction::Remote(remote::RemoteAction::Open), cx)
                    })),
            )
            .child(
                widgets::btn(t, "\u{1f310} Web")
                    .id("tb-web")
                    .tooltip(widgets::text_tip("Browser plugin: launch / install".into()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(DashAction::Browser(browser_ui::BrowserAction::Open), cx)
                    })),
            )
            .child(div().flex_1())
            .child(
                // Dev-toggle obscure (egui hides the HUD in a menu): clicking
                // the fleet-stats readout toggles the performance HUD.
                div()
                    .id("tb-stats")
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .font_family("JetBrains Mono")
                    .text_color(t.weak)
                    .child(stats_sub)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.dispatch(DashAction::PerfToggle, cx)),
                    ),
            )
            .child(
                widgets::btn(t, "MCP")
                    .id("tb-mcp")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            DashAction::Mgr(managers::MgrAction::Open(managers::MgrKind::Mcp)),
                            cx,
                        )
                    })),
            )
            .child(
                widgets::btn(t, "Skills")
                    .id("tb-skills")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            DashAction::Mgr(managers::MgrAction::Open(managers::MgrKind::Skills)),
                            cx,
                        )
                    })),
            )
            .child(
                widgets::btn(t, "Agents")
                    .id("tb-agents")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            DashAction::Mgr(managers::MgrAction::Open(managers::MgrKind::Agents)),
                            cx,
                        )
                    })),
            )
            .child(
                widgets::btn(t, "Models")
                    .id("tb-models")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            DashAction::Mgr(managers::MgrAction::Open(managers::MgrKind::Models)),
                            cx,
                        )
                    })),
            )
            .child(
                widgets::btn(t, "Config")
                    .id("tb-config")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dispatch(
                            DashAction::Mgr(managers::MgrAction::Open(managers::MgrKind::Config)),
                            cx,
                        )
                    })),
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
            .child(theme_ui::picker(
                t,
                &entity,
                &self.theme,
                &self
                    .themes
                    .iter()
                    .map(|p| p.name.clone())
                    .collect::<Vec<_>>(),
                self.theme_picker_open,
            ))
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let perf_began = self.perf.frame_begin();
        let t = self.tokens;
        let entity = cx.entity();

        // Presence heuristic input: is the window focused right now?
        self.window_active = window.is_window_active();
        // P5: track the window's placement so the drain-loop save can persist
        // it. The windowed/restore rect is carried by every WindowBounds arm.
        {
            let (b, maxed) = match window.window_bounds() {
                WindowBounds::Windowed(b) => (b, false),
                WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
            };
            self.win_rect = Some((
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ));
            self.win_maximized = maxed;
        }
        self.apply_terminal_resize();
        self.browser_embed_upkeep(window);
        self.dev_url_upkeep(cx);

        // One-shot: focus the composer when a chat was just opened.
        if let Some(id) = self.pending_focus.take()
            && let Some(input) = self.chat_inputs.get(&id)
        {
            window.focus(&input.read(cx).focus_handle(cx));
        }
        // One-shot: focus the tree-op name input when an op was just armed.
        if std::mem::take(&mut self.pending_tree_focus)
            && let Some(input) = &self.tree_op_input
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
        // Screen sanity (B13.1 class fix): a chat screen must have a live
        // workspace AND its input entity. Any path that forgets (a closed
        // workspace, a future jump-to-chat) degrades to the dashboard
        // instead of panicking the whole app.
        if let Screen::Chat(id) = self.screen {
            if self.supervisor.get(id).is_none() {
                self.screen = Screen::Dashboard;
            } else {
                self.ensure_chat_input(id, cx);
            }
        }
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
            self.browser_tab
                .map(|id| (self.browser.tab_title(id), self.screen == Screen::Browser)),
            &entity,
        );

        let body: gpui::AnyElement = match self.screen {
            Screen::Den => self.den_body(cx),
            Screen::Chat(id)
                if self.supervisor.get(id).is_none() || !self.chat_inputs.contains_key(&id) =>
            {
                // Unreachable in practice (the sanity pass above repairs
                // both); kept so a future slip degrades instead of
                // aborting the app (B13.1 was an `expect` here).
                self.screen = Screen::Dashboard;
                div().size_full().into_any_element()
            }
            Screen::Chat(id) => {
                // Keep the transcript pinned to the newest turn: scroll to the
                // bottom on first open and whenever a new entry arrives WHILE
                // already at the bottom — but never yank the view down if the
                // user has scrolled up to read history (#scroll fix).
                let len = self
                    .supervisor
                    .get(id)
                    .map(|w| w.entries().len())
                    .unwrap_or(0);
                let handle = self.chat_scroll.entry(id).or_default().clone();
                let seen = self.chat_seen_len.get(&id).copied();
                let at_bottom = {
                    let max = handle.max_offset().height;
                    max <= px(1.) || handle.offset().y <= -max + px(8.)
                };
                if seen.is_none() || (seen.is_some_and(|s| len > s) && at_bottom) {
                    handle.scroll_to_bottom();
                }
                self.chat_seen_len.insert(id, len);

                let ws = self.supervisor.get(id).expect("guarded by the arm above");
                let input = self
                    .chat_inputs
                    .get(&id)
                    .expect("guarded by the arm above")
                    .clone();
                // Chat surfaces speak as THIS workspace's puppy — for a
                // remote workspace that's the remote host's identity (B13.8).
                let ws_name = ws_puppy(ws);
                chat::chat_screen(&chat::ChatArgs {
                    t,
                    ws,
                    root: entity.clone(),
                    input,
                    style: self.composer_style,
                    pop: self.chat_pop.as_ref(),
                    puppy: ws_name.clone(),
                    user_avatar: self.user_avatar().to_string(),
                    puppy_avatar: self.puppy_avatar().to_string(),
                    creds_armed: self.creds_confirm == Some(id),
                    creds_busy: self
                        .creds_pending
                        .as_ref()
                        .is_some_and(|p| p.ws == Some(id)),
                    show_all: self.show_all_chat.contains(&id),
                    hidden_mode: self.hidden_mode,
                    expanded: &self.expanded_entries,
                    reduce_motion: self.reduce_motion,
                    scroll: handle,
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
                    markers: ws.tree_markers(),
                    tree_op_input: self.tree_op_input.as_ref(),
                    picker_path_input: self.picker_path_input.as_ref(),
                    // Header-initiated "new at root" op for THIS workspace,
                    // distinguished from per-row ops by the absent TreeMenu pop.
                    tree_root_new: match (&self.tree_op, &self.chat_pop) {
                        (Some(TreeOp::New(tid, _, is_dir)), pop)
                            if *tid == id && !matches!(pop, Some(ChatPop::TreeMenu(..))) =>
                        {
                            Some(*is_dir)
                        }
                        _ => None,
                    },
                    commit_input: self.commit_inputs.get(&id),
                    git_list_mode: self.git_list_mode.contains(&id),
                    graph_menu: self.graph_menu.as_ref(),
                    branch_input: self.branch_input.as_ref(),
                    branch_armed: self.branch_target.is_some(),
                    creds_user_input: self.creds_user_input.as_ref(),
                    creds_pass_input: self.creds_pass_input.as_ref(),
                    term_focus: &self.term_focus,
                    term_focused: self.term_focus.is_focused(window),
                    term_colors: &self.term_colors,
                    term_resize: self.term_resize.clone(),
                    dev_urls: &self.detected_dev_urls,
                    browser_available: self.browser.is_available(),
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
                            puppy: ws_name.clone(),
                            puppy_avatar: self.puppy_avatar().to_string(),
                        }
                    }),
                })
            }
            Screen::Dashboard => self.dashboard_body(cx),
            Screen::Browser => self.browser_body(cx),
        };

        let out = div()
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
            // Body takes the REMAINING height (not 100%, which would push its
            // scroll viewport's bottom below the window) so the dashboard
            // grid / chat transcript scroll correctly (F3). The wrapper is a
            // flex column so the body's own flex_1/size_full actually
            // stretches to fill it (a plain block would let a content-sized
            // body like the chat screen collapse, wasting the lower window).
            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
            .children(self.manager_open.map(|kind| {
                let ws = self.serving_ws();
                managers_ui::overlay(&managers_ui::MgrArgs {
                    t,
                    kind,
                    ws,
                    root: cx.entity(),
                    inputs: &self.mgr_inputs,
                    paste_input: self.mgr_paste_input.as_ref(),
                    filter: self
                        .mgr_inputs
                        .get(managers::F_FILTER)
                        .map(|i| i.read(cx).text().to_string())
                        .unwrap_or_default(),
                    tool_filter: self
                        .mgr_inputs
                        .get(managers::F_TOOLF)
                        .map(|i| i.read(cx).text().to_string())
                        .unwrap_or_default(),
                    selected: self.mgr_selected.as_deref(),
                    pending: &self.mgr_pending,
                    mcp_wizard: self.mcp_wizard.as_ref(),
                    skills_wizard: self.skills_wizard.as_ref(),
                    agent_wizard: self.agent_wizard.as_ref(),
                    agent_delete_confirm: self.agent_delete_confirm.as_deref(),
                    agent_model_menu: self.agent_model_menu,
                    models_editor: self.models_editor,
                    cfg_entries: &self.cfg_entries,
                    cfg_edit_key: self.cfg_edit_key.as_deref(),
                })
            }))
            // Agent Creator chat modal — floats above the agents manager (F8).
            .children(self.agent_creator_modal(&t, &cx.entity()))
            .children(self.remote.as_ref().map(|st| {
                remote_ui::overlay(
                    t,
                    &cx.entity(),
                    st,
                    &self.remote_inputs,
                    &self
                        .remote_inputs
                        .first()
                        .map(|i| i.read(cx).text().to_string())
                        .unwrap_or_default(),
                    &self
                        .remote_inputs
                        .get(1)
                        .map(|i| i.read(cx).text().to_string())
                        .unwrap_or_default(),
                    // Dialog-initiated pushes have no workspace attached.
                    self.creds_pending.as_ref().is_some_and(|p| p.ws.is_none()),
                )
            }))
            .children(self.theme_editor_open.then(|| {
                theme_editor_ui::editor_overlay(
                    t,
                    &cx.entity(),
                    &self.theme_inputs,
                    &self.theme_palette,
                    &self.terminal_theme,
                    &self
                        .themes
                        .iter()
                        .map(|p| p.name.clone())
                        .collect::<Vec<_>>(),
                    &self.theme,
                )
            }))
            .children(self.about.open.then(|| {
                let ver = self.cp_version_label();
                about::panel(&t, &self.about, ver, &entity)
            }))
            .children(self.avatar_ui.open.then(|| {
                avatars::panel(
                    &t,
                    &self.avatar_ui,
                    self.user_avatar(),
                    self.puppy_avatar(),
                    self.avatar_input.as_ref(),
                    &entity,
                )
            }))
            .children(
                self.perf
                    .visible
                    .then(|| perf_ui::hud(&t, &entity, &self.perf)),
            )
            .children(self.tree_context_menu(cx))
            .children(self.quit_confirm.then(|| self.quit_overlay(&entity)))
            .children(
                self.editor_close_confirm
                    .map(|(id, ix)| self.editor_close_overlay(id, ix, &entity)),
            )
            .child(widgets::toast_layer(&t, &self.toasts));
        self.perf.frame_end(perf_began);
        out
    }
}

impl RootView {
    /// Window-close gate (#4): allow the quit unless a puppy is mid-turn. The
    /// first attempt while busy arms the confirm overlay and vetoes the close;
    /// once the user picks "Quit anyway" (`quit_armed`), it sails through.
    fn allow_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.quit_armed {
            return true;
        }
        if self.supervisor.any_running_turn() {
            self.quit_confirm = true;
            cx.notify();
            return false;
        }
        true
    }

    /// Resolve the file path for an editor tab index, if it is a File tab.
    fn editor_tab_path(&self, id: WorkspaceId, ix: usize) -> Option<std::path::PathBuf> {
        self.supervisor
            .get(id)
            .and_then(|ws| match ws.editor_tabs().get(ix) {
                Some(crate::workspace::EditorItem::File(p)) => Some(p.clone()),
                _ => None,
            })
    }

    /// Discard a modified file's unsaved edits and close its tab. Drops the
    /// cached input entity so a later reopen rebuilds from the clean buffer.
    pub(crate) fn editor_discard_close(
        &mut self,
        id: WorkspaceId,
        ix: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.editor_tab_path(id, ix) {
            self.editor_inputs.remove(&(id.0, path.clone()));
            if let Some(ws) = self.supervisor.get_mut(id) {
                ws.discard_file(&path);
                ws.close_editor(ix);
            }
        }
        self.editor_close_confirm = None;
        cx.notify();
    }

    /// Save a modified file then close its tab. If the write fails we leave the
    /// tab open (the save error shows in its bar) and just drop the prompt.
    pub(crate) fn editor_save_close(&mut self, id: WorkspaceId, ix: usize, cx: &mut Context<Self>) {
        if let Some(path) = self.editor_tab_path(id, ix)
            && let Some(ws) = self.supervisor.get_mut(id)
            && ws.save_file(&path)
        {
            ws.close_editor(ix);
        }
        self.editor_close_confirm = None;
        cx.notify();
    }

    /// Centered "unsaved changes" modal for closing a dirty editor tab. Like
    /// `quit_overlay`, the scrim occludes everything; the backdrop cancels.
    fn editor_close_overlay(
        &self,
        id: WorkspaceId,
        ix: usize,
        root: &Entity<RootView>,
    ) -> AnyElement {
        let t = self.tokens;
        let name = self
            .editor_tab_path(id, ix)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "this file".to_string());
        let root_cancel = root.clone();
        let root_discard = root.clone();
        let root_save = root.clone();
        let panel = div()
            .id("ed-close-panel")
            .occlude()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(380.))
            .p_4()
            .rounded(px(14.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            // Don't let clicks on the panel reach the backdrop (which cancels).
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(t.text)
                    .child(format!("Unsaved changes in {name}")),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(t.weak)
                    .child("Save your changes before closing, or discard them?"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(widgets::btn(&t, "Cancel").id("ed-close-cancel").on_click(
                        move |_, _, cx| {
                            root_cancel.update(cx, |r, cx| {
                                r.editor_close_confirm = None;
                                cx.notify();
                            });
                        },
                    ))
                    .child(
                        widgets::btn(&t, "Discard\u{2009}&\u{2009}Close")
                            .id("ed-close-discard")
                            .on_click(move |_, _, cx| {
                                root_discard.update(cx, |r, cx| r.editor_discard_close(id, ix, cx));
                            }),
                    )
                    .child(
                        widgets::primary_btn(&t, "Save\u{2009}&\u{2009}Close")
                            .id("ed-close-save")
                            .on_click(move |_, _, cx| {
                                root_save.update(cx, |r, cx| r.editor_save_close(id, ix, cx));
                            }),
                    ),
            );
        let root_backdrop = root.clone();
        let scrim = div()
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(widgets::alpha(t.bg, 0.6))
            .id("ed-close-scrim")
            .on_click(move |_, _, cx| {
                root_backdrop.update(cx, |r, cx| {
                    r.editor_close_confirm = None;
                    cx.notify();
                });
            })
            .child(panel);
        gpui::deferred(scrim).with_priority(210).into_any_element()
    }

    /// The centered "a puppy is still working" quit confirmation (#4). Forces
    /// a choice — the scrim occludes everything behind it.
    fn quit_overlay(&self, root: &Entity<RootView>) -> AnyElement {
        let t = self.tokens;
        let root_keep = root.clone();
        let root_quit = root.clone();
        // Name the actual puppy (or count them when more than one is busy).
        let names = self.supervisor.running_turn_names();
        let headline = match names.as_slice() {
            [one] => format!("{one} is still running"),
            many => format!("{} puppies are still running", many.len()),
        };
        let scrim = div()
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(widgets::alpha(t.bg, 0.6))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(360.))
                    .p_4()
                    .rounded(px(14.))
                    .bg(t.panel)
                    .border_1()
                    .border_color(t.line_soft)
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(t.text)
                            .child(headline),
                    )
                    .child(div().text_size(px(12.5)).text_color(t.weak).child(
                        "Quitting now stops the running turn. \
                                 Close Doghouse anyway?",
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(widgets::btn(&t, "Keep running").id("quit-keep").on_click(
                                move |_, _, cx| {
                                    root_keep.update(cx, |r, cx| {
                                        r.quit_confirm = false;
                                        cx.notify();
                                    });
                                },
                            ))
                            .child(
                                widgets::primary_btn(&t, "Quit anyway")
                                    .id("quit-yes")
                                    .on_click(move |_, window, cx| {
                                        root_quit.update(cx, |r, _| r.quit_armed = true);
                                        window.remove_window();
                                    }),
                            ),
                    ),
            );
        gpui::deferred(scrim).with_priority(200).into_any_element()
    }

    /// The floating, cursor-anchored tree context menu (VSCode-style, F-req).
    /// Mounted at the window root so its click-away scrim covers everything;
    /// `gpui::anchored` keeps it on-screen near edges. The menu body itself
    /// lives with the explorer (`chat::tree_menu_panel`).
    fn tree_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (id, path, is_dir) = match &self.chat_pop {
            Some(ChatPop::TreeMenu(id, path, is_dir)) => (*id, path.clone(), *is_dir),
            _ => return None,
        };
        let pos = self.tree_menu_pos?;
        let entity = cx.entity();
        let delete_pending = self
            .tree_delete_confirm
            .as_ref()
            .filter(|(cid, ..)| *cid == id)
            .map(|(_, p, _)| p.clone());
        let panel = chat::tree_menu_panel(
            self.tokens,
            id,
            &path,
            is_dir,
            self.tree_op_input.as_ref(),
            self.tree_op.is_some(),
            delete_pending.as_deref(),
            &entity,
        );
        let scrim_root = entity.clone();
        Some(
            gpui::deferred(
                div()
                    .absolute()
                    .inset_0()
                    .child(
                        // Click-away scrim: closes the menu (and abandons any
                        // armed rename/new op so it can't leak elsewhere).
                        div()
                            .id("tree-menu-scrim")
                            .absolute()
                            .inset_0()
                            .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                                scrim_root
                                    .update(cx, |r, cx| r.dispatch(DashAction::TreeOpCancel, cx));
                            }),
                    )
                    .child(gpui::anchored().position(pos).snap_to_window().child(panel)),
            )
            .with_priority(260)
            .into_any_element(),
        )
    }

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
            .iter_visible()
            .map(|ws| dashboard::snapshot(ws, &t, self.model_popover == Some(ws.id), &puppy))
            .collect();
        let waiting: Vec<(WorkspaceId, String, Option<String>)> = self
            .supervisor
            .iter_visible()
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
                            .child(dashboard::pack_header(
                                &t,
                                &puppy,
                                self.puppy_avatar(),
                                &stats,
                                agg,
                                &entity,
                            ))
                            .child(browser_ui::plugins_section(
                                &t,
                                &entity,
                                &self.browser,
                                self.plugins_open,
                            ))
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
                                // Grid/List/Focus only affects the fleet below —
                                // it lives with the dashboard, not the global
                                // toolbar (B13.6; mock: right end of the row
                                // above the fleet).
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .child(div().flex_1())
                                        .child(dashboard::segmented(
                                            &t,
                                            self.dash_mode,
                                            &entity,
                                        )),
                                )
                                .child(dashboard::fleet(
                                    &t,
                                    self.dash_mode,
                                    cards,
                                    &entity,
                                    input_snap,
                                    &self.input_focus,
                                    self.reduce_motion,
                                    self.card_close_confirm,
                                ))
                            }),
                    ),
            )
            .into_any_element()
    }
}

/// A workspace's own puppy display name ("Puppy" until its sidecar
/// announces). Remote workspaces report the REMOTE host's puppy.
fn ws_puppy(ws: &crate::workspace::Workspace) -> String {
    if ws.puppy_name.is_empty() {
        "Puppy".to_string()
    } else {
        ws.puppy_name.clone()
    }
}

/// The app-headline puppy: first LOCAL workspace with a real announced name.
/// Remote announcements never become the headline (B13.8) — pure so the
/// pinning rule is unit-testable without a supervisor.
fn headline_puppy<'a>(names: impl Iterator<Item = (&'a str, bool)>) -> &'a str {
    let mut found = "Puppy";
    for (name, is_remote) in names {
        if !is_remote && !name.is_empty() && name != "Puppy" {
            found = name;
            break;
        }
    }
    found
}

#[cfg(test)]
mod identity_tests {
    use super::headline_puppy;

    #[test]
    fn remote_announcement_never_becomes_the_headline() {
        // A remote sidecar reports first — the headline must not adopt it.
        let names = [("Bandit", true), ("Rex", false)];
        assert_eq!(headline_puppy(names.into_iter()), "Rex");
        // Only remotes reporting: stay on the default rather than borrow a
        // remote identity.
        let only_remote = [("Bandit", true), ("Fido", true)];
        assert_eq!(headline_puppy(only_remote.into_iter()), "Puppy");
    }

    #[test]
    fn local_default_and_empty_names_are_skipped() {
        let names = [("", false), ("Puppy", false), ("Rex", false)];
        assert_eq!(headline_puppy(names.into_iter()), "Rex");
        assert_eq!(headline_puppy(std::iter::empty()), "Puppy");
    }
}
