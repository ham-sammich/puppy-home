//! Frontend-agnostic UI waking.
//!
//! Backend threads (sidecar readers, pack client, PTY reader, ...) need to
//! nudge the UI when new data arrives, but must not depend on a particular
//! GUI toolkit. They hold an `Arc<dyn UiWaker>` and call [`UiWaker::wake`];
//! each frontend supplies the implementation (for egui, a `request_repaint`).

use std::sync::Arc;

/// Wakes the UI so it repaints and drains pending events. Implementations
/// must be cheap, idempotent, and callable from any thread.
pub trait UiWaker: Send + Sync {
    fn wake(&self);
}

/// The egui frontend's waker: a wake is a `request_repaint`.
pub struct EguiWaker(pub eframe::egui::Context);

impl UiWaker for EguiWaker {
    fn wake(&self) {
        self.0.request_repaint();
    }
}

/// Wrap an `egui::Context` as a shareable waker (for egui-side call sites).
pub fn egui_waker(ctx: &eframe::egui::Context) -> Arc<dyn UiWaker> {
    Arc::new(EguiWaker(ctx.clone()))
}

/// A waker that does nothing — for headless tests.
#[cfg(test)]
pub struct NoopWaker;

#[cfg(test)]
impl UiWaker for NoopWaker {
    fn wake(&self) {}
}
