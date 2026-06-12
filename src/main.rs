//! puppy-home — a native windowed IDE-like shell around Code Puppy.
//!
//! The window supervises one Code Puppy sidecar per workspace and presents
//! dockable views (chat, dashboard, …). See `app`, `supervisor`, `workspace`.
//!
//! On Windows, `windows_subsystem = "windows"` (in release builds) suppresses
//! the console window so it launches as a clean GUI app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// ---------------------------------------------------------------------------
// redesign/gpui: GPUI is the frontend. The egui-coupled modules stay in the
// tree and KEEP COMPILING (eframe remains a dependency for their types) so
// their reusable logic can be extracted incrementally — but with the
// `egui-shell` feature off (the default) nothing runs them, so they are
// allow(dead_code) to keep the build signal clean. Modules the GPUI frontend
// actually drives (backend, supervisor, waker, gpui_ui) keep full lints.
// ---------------------------------------------------------------------------
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod app;
mod backend;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod browser;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod dock_layout;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod fonts;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod git;
mod gpui_ui;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod pack;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod perf;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod plugin;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod proc;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod session;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod shell;
mod supervisor;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod terminal;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod theme;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod views;
mod waker;
#[cfg_attr(not(feature = "egui-shell"), allow(dead_code))]
mod workspace;

/// Default on this branch: the GPUI shell.
#[cfg(not(feature = "egui-shell"))]
fn main() {
    install_panic_logger();
    gpui_ui::run();
}

/// Legacy entry: `cargo run --features egui-shell` launches the eframe app.
#[cfg(feature = "egui-shell")]
fn main() -> eframe::Result<()> {
    use eframe::egui;

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
