//! Frontend-agnostic UI waking.
//!
//! Backend threads (sidecar readers, pack client, PTY reader, ...) need to
//! nudge the UI when new data arrives, but must not depend on a particular
//! GUI toolkit. They hold an `Arc<dyn UiWaker>` and call [`UiWaker::wake`];
//! the GPUI shell supplies the implementation ([`GpuiWaker`] in `gpui_ui`).

/// Wakes the UI so it repaints and drains pending events. Implementations
/// must be cheap, idempotent, and callable from any thread.
pub trait UiWaker: Send + Sync {
    fn wake(&self);
}

/// A waker that does nothing — for headless tests.
#[cfg(test)]
pub struct NoopWaker;

#[cfg(test)]
impl UiWaker for NoopWaker {
    fn wake(&self) {}
}
