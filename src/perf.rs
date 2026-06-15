//! Lightweight performance HUD: per-frame CPU cost, update rate, and process
//! memory. Toggled from the app top bar; costs ~nothing while hidden.

use std::collections::VecDeque;

/// Rolling sample window (frames). (pub(crate): shared with the GPUI HUD.)
pub(crate) const WINDOW: usize = 240;

pub(crate) fn push(buf: &mut VecDeque<f32>, v: f32) {
    if buf.len() >= WINDOW {
        buf.pop_front();
    }
    buf.push_back(v);
}

pub(crate) fn mean(buf: &VecDeque<f32>) -> f32 {
    if buf.is_empty() {
        0.0
    } else {
        buf.iter().sum::<f32>() / buf.len() as f32
    }
}

pub(crate) fn peak(buf: &VecDeque<f32>) -> f32 {
    buf.iter().copied().fold(0.0, f32::max)
}

pub(crate) fn fmt_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

/// (working set, private/pagefile) in bytes; zeros when unavailable.
#[cfg(windows)]
pub(crate) fn process_memory() -> (u64, u64) {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb).as_bool() {
            (
                counters.WorkingSetSize as u64,
                counters.PagefileUsage as u64,
            )
        } else {
            (0, 0)
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn process_memory() -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_caps_window() {
        let mut buf = VecDeque::new();
        for i in 0..(WINDOW + 50) {
            push(&mut buf, i as f32);
        }
        assert_eq!(buf.len(), WINDOW);
        assert_eq!(*buf.front().unwrap(), 50.0);
    }

    #[test]
    fn mean_and_peak() {
        let buf: VecDeque<f32> = [1.0, 2.0, 3.0].into_iter().collect();
        assert!((mean(&buf) - 2.0).abs() < f32::EPSILON);
        assert_eq!(peak(&buf), 3.0);
        assert_eq!(mean(&VecDeque::new()), 0.0);
    }

    #[test]
    fn fmt_bytes_units() {
        assert_eq!(fmt_bytes(10 * 1024 * 1024), "10.0 MB");
        assert_eq!(fmt_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");
    }
}
