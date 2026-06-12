//! The GPUI frontend (Task 2.1 scaffold): one window, design tokens from the
//! shared amber palette, bundled fonts, and a live Supervisor feed.
//!
//! This deliberately renders an UGLY bare-bones workspace list — the point of
//! this module is to prove the plumbing (waker → drain loop → notify → render
//! with live sidecar data), not the design. The real Command Center lands in
//! Task 2.2+ on top of exactly this skeleton.
//!
//! ## The frame-loop pattern (template for everything after — see GPUI_NOTES.md)
//! GPUI is retained/reactive: nothing re-renders until an entity calls
//! `cx.notify()`. Backend threads signal through [`GpuiWaker`] (a coalescing
//! channel); a single foreground task selects on `{wake, adaptive timer}`,
//! folds events via `Supervisor::drain()`, and notifies the root view. The
//! timer runs ~250ms while any workspace is busy (status polls, elapsed
//! timers) and relaxes to ~1s when idle.

mod assets;
mod tokens;
mod waker;

use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Application, Bounds, Context, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

use crate::supervisor::Supervisor;
use crate::workspace::Workspace;
use tokens::Tokens;
use waker::GpuiWaker;

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
            let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
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

pub struct RootView {
    supervisor: Supervisor,
    tokens: Tokens,
    /// Most recent open/spawn error, shown inline (scaffold-grade UX).
    last_error: Option<String>,
    /// `PUPPY_GPUI_PROMPT`: a one-shot prompt fired at the first ready
    /// sidecar — probe instrumentation to watch tokens tick end-to-end.
    probe_prompt: Option<String>,
}

impl RootView {
    fn new(cx: &mut Context<Self>) -> Self {
        let (waker, wake_rx) = GpuiWaker::new();
        let mut supervisor = Supervisor::new(waker);
        let mut last_error = None;

        // Headless-ish probe: auto-open a folder from the environment so the
        // plumbing can be exercised (and logged) without clicking around.
        if let Some(root) = std::env::var_os("PUPPY_GPUI_OPEN") {
            if let Err(e) = supervisor.open(root.into()) {
                last_error = Some(e);
            }
        }

        Self::spawn_drain_loop(cx, wake_rx);
        RootView {
            supervisor,
            tokens: Tokens::dark(),
            last_error,
            probe_prompt: std::env::var("PUPPY_GPUI_PROMPT").ok(),
        }
    }

    /// Fire the probe prompt once the first sidecar reports ready.
    fn maybe_send_probe_prompt(&mut self) {
        let Some(prompt) = &self.probe_prompt else {
            return;
        };
        let Some(id) = self
            .supervisor
            .iter()
            .find(|w| w.is_ready())
            .map(|w| w.id)
        else {
            return;
        };
        let prompt = prompt.clone();
        if let Some(ws) = self.supervisor.get_mut(id) {
            eprintln!("[probe] sending prompt to {}: {prompt:?}", ws.name);
            // Through the real state machine (transcript, `running`, status
            // polling) — NOT a raw backend.send_prompt, which would skip the
            // polling that feeds token counts.
            ws.send_user_prompt(&prompt);
            self.probe_prompt = None;
        }
    }

    /// The recurring drain task: wake-driven with an adaptive timer floor.
    fn spawn_drain_loop(cx: &mut Context<Self>, mut wake_rx: UnboundedReceiver<()>) {
        let probe = std::env::var_os("PUPPY_GPUI_PROBE").is_some();
        let mut last_probe = String::new();
        cx.spawn(async move |this, cx| {
            loop {
                // Root gone (app shutting down) → the loop dies with it.
                let Ok(busy) = this.update(cx, |root, cx| {
                    root.supervisor.drain();
                    root.maybe_send_probe_prompt();
                    cx.notify();
                    if probe {
                        let line = root.probe_line();
                        if line != last_probe {
                            eprintln!("[probe] {line}");
                            last_probe = line;
                        }
                    }
                    root.supervisor.any_busy()
                }) else {
                    return;
                };

                let cadence = if busy { DRAIN_BUSY } else { DRAIN_IDLE };
                let timer = cx.background_executor().timer(cadence);
                futures::select_biased! {
                    _ = wake_rx.next() => {}
                    _ = futures::FutureExt::fuse(timer) => {}
                }
                // Coalesce any burst of wakes into the single drain above.
                while let Ok(()) = wake_rx.try_recv() {}
            }
        })
        .detach();
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

    /// A bare-bones live row: status dot, dir name, status label, token count.
    fn workspace_row(&self, ws: &Workspace) -> impl IntoElement {
        let t = &self.tokens;
        let status = t.status_color(ws.status);
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .bg(t.card)
            .rounded(px(10.))
            .border_1()
            .border_color(t.line_soft)
            .child(div().size(px(9.)).rounded_full().bg(status))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(ws.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.weak)
                            .font_family("JetBrains Mono")
                            .child(ws.root.display().to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(status)
                    .child(ws.status.label().to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(t.weak)
                    .font_family("JetBrains Mono")
                    .child(format!(
                        "{} tok \u{b7} {:.1}/s",
                        ws.total_tokens, ws.token_rate
                    )),
            )
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.tokens;
        let rows: Vec<_> = self
            .supervisor
            .iter()
            .map(|ws| self.workspace_row(ws))
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(14.))
            .font_family("Space Grotesk")
            .child(
                // Header: title + Open Folder.
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Code Puppy"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("open-folder")
                            .px_3()
                            .py_1()
                            .bg(t.card)
                            .rounded(px(8.))
                            .border_1()
                            .border_color(t.line_soft)
                            .text_color(t.accent)
                            .cursor_pointer()
                            .hover(|s| s.border_color(t.accent))
                            .child("Open Folder\u{2026}")
                            .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
                    ),
            )
            .when_some(self.last_error.clone(), |el, err| {
                el.child(div().text_sm().text_color(t.error).child(err))
            })
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .text_color(t.weak)
                        .child("No workspaces \u{2014} open a folder to spawn a Code Puppy."),
                )
            })
            .children(rows)
    }
}
