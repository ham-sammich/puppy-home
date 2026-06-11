//! Child-process spawning helpers.

use std::process::Command;

/// Prevent a child process from opening its own console window.
///
/// The release app is a GUI (`windows_subsystem = "windows"`), so unlike the
/// dev build there is no parent console for children to inherit — every
/// spawned `git`/`python` would otherwise flash (or keep) an empty terminal.
/// `CREATE_NO_WINDOW` suppresses that. No-op on other platforms, and harmless
/// for processes whose stdio we pipe anyway.
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}
