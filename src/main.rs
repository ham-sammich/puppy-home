//! puppy-home — a native windowed IDE-like shell around Code Puppy.
//!
//! The window supervises one Code Puppy sidecar per workspace and presents
//! dockable views (chat, dashboard, …). See `app`, `supervisor`, `workspace`.
//!
//! On Windows, `windows_subsystem = "windows"` (in release builds) suppresses
//! the console window so it launches as a clean GUI app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod fonts;
mod git;
mod session;
mod shell;
mod supervisor;
mod terminal;
mod views;
mod workspace;

use eframe::egui;

fn main() -> eframe::Result<()> {
    install_panic_logger();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([480.0, 360.0])
            .with_title("puppy-home"),
        ..Default::default()
    };

    eframe::run_native(
        "puppy-home",
        native_options,
        Box::new(|cc| Ok(Box::new(app::PuppyApp::new(cc)))),
    )
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
