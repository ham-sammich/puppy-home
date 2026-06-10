//! Native-window embedding: reparent the browser plugin's OS window into the
//! host window so the webview renders *inside* the Browser tab.
//!
//! Only Windows is wired today (via the `windows` crate). Other platforms get
//! no-op stubs, so the browser simply stays in its own window there.

/// Whether native embedding is supported on this platform.
pub const SUPPORTED: bool = cfg!(windows);

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE,
        SWP_NOCOPYBITS, SWP_NOZORDER, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        WS_CHILD, WS_POPUP, WS_VISIBLE,
    };

    fn hwnd(raw: i64) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    /// Make `child` a borderless child of `parent` (call once per window).
    pub fn reparent(parent: i64, child: i64) {
        unsafe {
            // Swap the top-level popup style for a child style.
            let style = GetWindowLongPtrW(hwnd(child), GWL_STYLE) as u32;
            let new = (style & !WS_POPUP.0) | WS_CHILD.0 | WS_VISIBLE.0;
            SetWindowLongPtrW(hwnd(child), GWL_STYLE, new as isize);
            let _ = SetParent(hwnd(child), hwnd(parent));
        }
    }

    /// Move/resize the child to (x, y, w, h) in parent-client pixels. Call only
    /// when the rect actually changes — repositioning every frame is choppy.
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

#[cfg(not(windows))]
mod imp {
    pub fn reparent(_parent: i64, _child: i64) {}
    pub fn place(_child: i64, _x: i32, _y: i32, _w: i32, _h: i32) {}
    pub fn show(_child: i64) {}
    pub fn hide(_child: i64) {}
    pub fn focus_host(_parent: i64, _child: i64) {}
}

pub use imp::{focus_host, hide, place, reparent, show};
