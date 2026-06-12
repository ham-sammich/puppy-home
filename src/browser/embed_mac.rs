//! macOS host-window introspection for GPUI in-tab browser embedding.
//!
//! GPUI exposes the native NSView via `raw-window-handle`
//! (`RawWindowHandle::AppKit`, pinned v0.199.10 `platform/mac/window.rs`).
//! From that single pointer Cocoa gives us everything the plugin's `embed`
//! command needs: the host's `windowNumber` (z-order anchor) and the
//! view's rect in GLOBAL TOP-LEFT PHYSICAL pixels — the exact convention
//! tao's `set_outer_position` consumes on the plugin side (and the same
//! one the egui shell sends from eframe's `inner_rect * ppp`).
//!
//! We deliberately do the conversion in Cocoa (`convertRect:toView:nil` →
//! `convertRectToScreen:` → flip against the PRIMARY screen height) rather
//! than trusting gpui's `Window::bounds()`, whose origin is relative to
//! the window's CURRENT screen — a different convention that breaks on
//! multi-display setups.

#![cfg(target_os = "macos")]

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

#[repr(C)]
#[derive(Clone, Copy)]
struct NsPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

/// The host NSWindow's `windowNumber` (the plugin orders itself above it).
/// Returns None when the view isn't in a window (shouldn't happen mid-render).
pub fn window_number(ns_view: *mut std::ffi::c_void) -> Option<i64> {
    if ns_view.is_null() {
        return None;
    }
    unsafe {
        let view = ns_view as *mut Object;
        let win: *mut Object = msg_send![view, window];
        if win.is_null() {
            return None;
        }
        let num: isize = msg_send![win, windowNumber];
        Some(num as i64)
    }
}

/// Whether the host window is miniaturized (the overlay must hide then —
/// it's a separate OS window and would squat on the desktop otherwise).
pub fn is_miniaturized(ns_view: *mut std::ffi::c_void) -> bool {
    if ns_view.is_null() {
        return false;
    }
    unsafe {
        let view = ns_view as *mut Object;
        let win: *mut Object = msg_send![view, window];
        if win.is_null() {
            return false;
        }
        let mini: bool = msg_send![win, isMiniaturized];
        mini
    }
}

/// Convert a rect in VIEW coordinates (gpui element bounds: logical px,
/// top-left origin, relative to the native view) into GLOBAL TOP-LEFT
/// PHYSICAL pixels for the plugin's `embed` command.
pub fn view_rect_to_screen_px(
    ns_view: *mut std::ffi::c_void,
    elem: (f32, f32, f32, f32),
) -> Option<(i32, i32, i32, i32)> {
    if ns_view.is_null() {
        return None;
    }
    unsafe {
        let view = ns_view as *mut Object;
        let win: *mut Object = msg_send![view, window];
        if win.is_null() {
            return None;
        }
        // The view's frame in Cocoa GLOBAL coords (bottom-left origin).
        let bounds: NsRect = msg_send![view, bounds];
        let nil_view: *mut Object = std::ptr::null_mut();
        let in_window: NsRect = msg_send![view, convertRect: bounds toView: nil_view];
        let on_screen: NsRect = msg_send![win, convertRectToScreen: in_window];

        // Flip to top-left global using the PRIMARY screen's height — the
        // anchor of Cocoa's global space and of tao's top-left space alike.
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let primary: *mut Object = msg_send![screens, firstObject];
        if primary.is_null() {
            return None;
        }
        let primary_frame: NsRect = msg_send![primary, frame];
        let view_top_logical =
            primary_frame.size.height - (on_screen.origin.y + on_screen.size.height);

        // gpui element coords are y-down from the view's top-left; Cocoa's
        // flipped result above is too, so they add directly.
        let (ex, ey, ew, eh) = elem;
        let x_logical = on_screen.origin.x + ex as f64;
        let y_logical = view_top_logical + ey as f64;

        let scale: f64 = msg_send![win, backingScaleFactor];
        Some((
            (x_logical * scale).round() as i32,
            (y_logical * scale).round() as i32,
            (ew as f64 * scale).round() as i32,
            (eh as f64 * scale).round() as i32,
        ))
    }
}
