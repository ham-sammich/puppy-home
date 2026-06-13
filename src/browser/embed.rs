//! Native-window embedding: glue the browser plugin's OS window over the
//! host window's Browser tab so the webview renders *inside* the app.
//!
//! Windows uses an OWNED top-level overlay, NOT `SetParent`/`WS_CHILD`:
//! the GPUI window is a DirectComposition surface
//! (`WS_EX_NOREDIRECTIONBITMAP` — G3 probe), and DComp content composes
//! OVER child HWNDs, so a reparented webview renders invisibly behind the
//! UI (G3 finding B1). An owned borderless popup rides above its owner in
//! z-order and auto-hides when the owner minimizes — the same glued-
//! overlay architecture the macOS path uses (over IPC there).
//!
//! Other platforms get no-op stubs, so the browser simply stays in its
//! own window there.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use windows::Win32::Foundation::{BOOL, HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWLP_HWNDPARENT, GetWindowLongPtrW, GetWindowThreadProcessId, SW_HIDE,
        SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW,
    };

    fn hwnd(raw: i64) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    /// Attach `child` to `parent` as an OWNED borderless overlay (call once
    /// per window). Owner, not parent: `WS_CHILD` under a DComp host window
    /// is composed over and never visible (see module docs).
    ///
    /// Deliberately does NOT touch `GWL_STYLE`: tao implements "borderless"
    /// by KEEPING `WS_CAPTION`/`WS_THICKFRAME` and eating the frame in
    /// `WM_NCCALCSIZE`. Stripping those bits manually desynced tao's state
    /// machine — `set_decorations(true)` on pop-out flipped tao's flag but
    /// the style bits it relies on were gone, leaving an immovable window
    /// with no title bar (G3 retest 3).
    pub fn attach(parent: i64, child: i64) {
        unsafe {
            // Tool window while embedded: an in-app canvas must not get its
            // own taskbar button (G3 retest 2: "two taskbar icons").
            let ex = GetWindowLongPtrW(hwnd(child), GWL_EXSTYLE) as u32;
            let ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0;
            SetWindowLongPtrW(hwnd(child), GWL_EXSTYLE, ex as isize);
            // Owned by the host: keeps us above it in z-order and hides us
            // when it minimizes.
            SetWindowLongPtrW(hwnd(child), GWLP_HWNDPARENT, parent as isize);
            // MSDN: long-ptr edits only take effect after a FRAMECHANGED pos.
            let _ = SetWindowPos(
                hwnd(child),
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    /// Convert a point in `parent`'s client pixels to screen pixels — the
    /// owned overlay is positioned in screen coords, but the canvas rect
    /// arrives in client px (same DPI math as the old child path).
    pub fn screen_origin(parent: i64, x: i32, y: i32) -> (i32, i32) {
        unsafe {
            let mut pt = POINT { x, y };
            let _ = ClientToScreen(hwnd(parent), &mut pt);
            (pt.x, pt.y)
        }
    }

    /// Move/resize the overlay to (x, y, w, h) in SCREEN pixels (see
    /// [`screen_origin`]). Call only when the rect actually changes —
    /// repositioning every frame is choppy.
    pub fn place(child: i64, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(
                hwnd(child),
                HWND::default(),
                x,
                y,
                w,
                h,
                // NOCOPYBITS avoids smearing stale pixels while resizing.
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
        }
    }

    /// Show the child without stealing focus from the egui surface.
    pub fn show(child: i64) {
        unsafe {
            let _ = ShowWindow(hwnd(child), SW_SHOWNA);
        }
    }

    /// Hide the child (e.g. when its tab isn't the active one).
    pub fn hide(child: i64) {
        unsafe {
            let _ = ShowWindow(hwnd(child), SW_HIDE);
        }
    }

    /// Release the overlay back to a free-standing window (the GPUI
    /// pop-out path; inverse of `attach`). Style bits are tao's business
    /// (see [`attach`]) — we only undo what attach did.
    pub fn unparent(child: i64) {
        unsafe {
            // A free-floating browser is a real app window again: give it
            // its taskbar button back.
            let ex = GetWindowLongPtrW(hwnd(child), GWL_EXSTYLE) as u32;
            let ex = (ex & !WS_EX_TOOLWINDOW.0) | WS_EX_APPWINDOW.0;
            SetWindowLongPtrW(hwnd(child), GWL_EXSTYLE, ex as isize);
            // Drop the owner link so the floating window z-orders freely.
            SetWindowLongPtrW(hwnd(child), GWLP_HWNDPARENT, 0);
            // FRAMECHANGED applies the ex-style edits; no NOZORDER so the
            // freed window surfaces at the top instead of staying put.
            let _ = SetWindowPos(
                hwnd(child),
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    /// Background glue: keeps the owned overlay tracking the host's canvas
    /// rect even while GPUI's render loop is starved — Windows modal
    /// move/size loops don't render, so render-driven placement visibly
    /// trailed window drags (G3 retest 3). Polls at ~80 Hz but only calls
    /// `SetWindowPos` when the target screen rect actually changed.
    pub struct Gluer {
        /// Glued rect in host-client px; `None` pauses tracking (hidden).
        target: Arc<Mutex<Option<(i32, i32, i32, i32)>>>,
        stop: Arc<AtomicBool>,
    }

    impl Gluer {
        pub fn spawn(parent: i64, child: i64) -> Self {
            let target: Arc<Mutex<Option<(i32, i32, i32, i32)>>> = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            {
                let target = Arc::clone(&target);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let mut last = None;
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(12));
                        let Some((x, y, w, h)) = *target.lock().unwrap() else {
                            continue;
                        };
                        let (sx, sy) = screen_origin(parent, x, y);
                        let now = (sx, sy, w, h);
                        if last != Some(now) {
                            place(child, sx, sy, w, h);
                            last = Some(now);
                        }
                    }
                });
            }
            Self { target, stop }
        }

        /// Update the glued client-px rect (`None` pauses, e.g. hidden tab).
        pub fn set(&self, rect: Option<(i32, i32, i32, i32)>) {
            *self.target.lock().unwrap() = rect;
        }
    }

    impl Drop for Gluer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Reclaim OS keyboard focus to the host window from the (cross-process)
    /// webview child. The child lives in the plugin's thread, so we briefly
    /// attach input queues to move focus across the thread boundary.
    pub fn focus_host(parent: i64, child: i64) {
        unsafe {
            let our = GetCurrentThreadId();
            let child_thread = GetWindowThreadProcessId(hwnd(child), None);
            if child_thread != 0 && child_thread != our {
                let _ = AttachThreadInput(our, child_thread, BOOL(1));
                let _ = SetFocus(hwnd(parent));
                let _ = AttachThreadInput(our, child_thread, BOOL(0));
            } else {
                let _ = SetFocus(hwnd(parent));
            }
        }
    }
}

// Non-Windows: the browser is embedded via a borderless overlay window driven
// over IPC (see `host.rs`/`mod.rs`), so these reparenting hooks are no-ops.
// `focus_host` is still called (to no effect); the rest are unused here.
#[cfg(not(windows))]
#[allow(dead_code)]
mod imp {
    pub fn attach(_parent: i64, _child: i64) {}
    pub fn unparent(_child: i64) {}
    pub fn screen_origin(_parent: i64, x: i32, y: i32) -> (i32, i32) {
        (x, y)
    }
    pub fn place(_child: i64, _x: i32, _y: i32, _w: i32, _h: i32) {}
    pub fn show(_child: i64) {}
    pub fn hide(_child: i64) {}
    pub fn focus_host(_parent: i64, _child: i64) {}
}

// On non-Windows only `focus_host` is used (as a no-op); the rest are part of
// the uniform embed API but unused there.
#[allow(unused_imports)]
pub use imp::{attach, focus_host, hide, place, screen_origin, show, unparent};
#[cfg(windows)]
pub use imp::Gluer;
