//! The GPUI frontend's [`UiWaker`]: backend threads nudge the UI through a
//! coalescing channel that the root view's drain loop awaits.
//!
//! GPUI's foreground executor is `!Send`, so backend threads can't poke an
//! entity directly. Instead the waker pushes a unit onto an unbounded channel
//! (cheap, lock-free, callable from any thread); the drain loop in
//! `gpui_ui::RootView` selects on `{wake, timer}` and collapses any burst of
//! wakes into a single `drain + notify`. See GPUI_NOTES.md for the pattern.

use std::sync::Arc;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::waker::UiWaker;

pub struct GpuiWaker {
    tx: UnboundedSender<()>,
}

impl GpuiWaker {
    /// Build the waker plus the receiver the drain loop listens on.
    pub fn new() -> (Arc<GpuiWaker>, UnboundedReceiver<()>) {
        let (tx, rx) = unbounded();
        (Arc::new(GpuiWaker { tx }), rx)
    }
}

impl UiWaker for GpuiWaker {
    fn wake(&self) {
        // Unbounded send never blocks; a closed receiver (app shutting down)
        // is fine to ignore — there is nobody left to wake.
        let _ = self.tx.unbounded_send(());
    }
}
