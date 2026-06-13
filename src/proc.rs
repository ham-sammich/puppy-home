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

/// Reveal a path in the OS file manager: select the entry where supported
/// (Windows Explorer, macOS Finder), otherwise open its containing folder.
/// Best-effort — failures are swallowed (it's a convenience action).
pub fn reveal_in_file_manager(path: &std::path::Path, _is_dir: bool) {
    #[cfg(windows)]
    {
        // `/select,` highlights the file/folder within its parent window.
        let mut cmd = Command::new("explorer");
        cmd.arg(format!("/select,{}", path.display()));
        hide_console(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No universal "select" on Linux; open the containing directory.
        let target = if _is_dir {
            path.to_path_buf()
        } else {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.to_path_buf())
        };
        let _ = Command::new("xdg-open").arg(target).spawn();
    }
}
