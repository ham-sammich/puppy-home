//! Native-window embedding: reparent the browser plugin's OS window into the
//! host window so the webview renders *inside* the Browser tab.
//!
//! Only Windows is wired today (via the `windows` crate). Other platforms get
//! no-op stubs, so the browser simply stays in its own window there.

/// Whether native embedding is supported on this platform.
pub const SUPPORTED: bool = cfg!(windows);

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SetParent,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_CHILD, WS_POPUP, WS_VISIBLE,
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

    /// Position + show the child at (x, y, w, h) in parent-client pixels.
    pub fn place(child: i64, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(
                hwnd(child),
                HWND::default(),
                x,
                y,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // SHOWNA = show without stealing focus from the egui surface.
            let _ = ShowWindow(hwnd(child), SW_SHOWNA);
        }
    }

    /// Hide the child (e.g. when its tab isn't the active one).
    pub fn hide(child: i64) {
        unsafe {
            let _ = ShowWindow(hwnd(child), SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn reparent(_parent: i64, _child: i64) {}
    pub fn place(_child: i64, _x: i32, _y: i32, _w: i32, _h: i32) {}
    pub fn hide(_child: i64) {}
}

pub use imp::{hide, place, reparent};
