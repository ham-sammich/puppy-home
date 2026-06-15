//! puppy-home — a native windowed IDE-like shell around Code Puppy.
//!
//! The window supervises one Code Puppy sidecar per workspace and presents
//! dockable views (chat, dashboard, …). See `app`, `supervisor`, `workspace`.
//!
//! On Windows, `windows_subsystem = "windows"` (in release builds) suppresses
//! the console window so it launches as a clean GUI app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// GPUI is THE frontend. These modules hold the frontend-agnostic core
// (backend, supervisor, git, terminal, session, ...) plus the GPUI shell
// itself (`gpui_ui`). The legacy egui shell was removed in Phase G5.
mod backend;
mod browser;
mod git;
mod gpui_ui;
mod pack;
mod perf;
mod plugin;
mod proc;
mod session;
mod supervisor;
mod terminal;
mod theme;
mod views;
mod waker;
mod workspace;

fn main() {
    install_panic_logger();
    gpui_ui::run();
}

/// Append any panic (message + backtrace) to `%LOCALAPPDATA%\puppy-home\crash.log`
/// so GUI crashes leave a trace even with no console attached.
fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let backtrace = std::backtrace::Backtrace::force_capture();
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("puppy-home");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("crash.log"))
        {
            let _ = writeln!(file, "=== panic ===\n{info}\n{backtrace}\n");
        }
        previous(info);
    }));
}
