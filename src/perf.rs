//! Lightweight performance HUD: per-frame CPU cost, update rate, and process
//! memory. Toggled from the app top bar; costs ~nothing while hidden.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui;

/// Rolling sample window (frames).
const WINDOW: usize = 240;

pub struct PerfStats {
    pub visible: bool,
    started: Instant,
    last_update: Option<Instant>,
    /// Wall-clock gap between successive `update()` calls (event-driven UI,
    /// so this measures repaint *rate*, not capability).
    update_gaps: VecDeque<f32>,
    /// CPU seconds spent producing each frame (reported by eframe).
    frame_costs: VecDeque<f32>,
    mem_polled: Option<Instant>,
    working_set: u64,
    private_bytes: u64,
}

impl Default for PerfStats {
    fn default() -> Self {
        Self {
            visible: false,
            started: Instant::now(),
            last_update: None,
            update_gaps: VecDeque::with_capacity(WINDOW),
            frame_costs: VecDeque::with_capacity(WINDOW),
            mem_polled: None,
            working_set: 0,
            private_bytes: 0,
        }
    }
}

impl PerfStats {
    /// Call once per `update()`.
    pub fn on_frame(&mut self, frame: &eframe::Frame) {
        let now = Instant::now();
        if let Some(last) = self.last_update.replace(now) {
            push(&mut self.update_gaps, (now - last).as_secs_f32());
        }
        if let Some(cpu) = frame.info().cpu_usage {
            push(&mut self.frame_costs, cpu);
        }
        // Memory is an OS call — poll at 1 Hz, and only while the HUD is open.
        if self.visible
            && self
                .mem_polled
                .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            self.mem_polled = Some(now);
            let (ws, pv) = process_memory();
            self.working_set = ws;
            self.private_bytes = pv;
        }
    }

    /// The HUD window (no-op while hidden).
    pub fn render(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        let avg_ms = mean(&self.frame_costs) * 1000.0;
        let max_ms = peak(&self.frame_costs) * 1000.0;
        let budget = 1000.0 / 60.0; // 16.7ms = 60fps budget
        let ups = match mean(&self.update_gaps) {
            g if g > 0.0 => 1.0 / g,
            _ => 0.0,
        };

        let mut open = true;
        egui::Window::new("Performance")
            .open(&mut open)
            .default_width(260.0)
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 36.0])
            .show(ctx, |ui| {
                egui::Grid::new("perf-grid").num_columns(2).show(ui, |ui| {
                    ui.label("frame cost (avg)");
                    ui.colored_label(cost_color(ui, avg_ms, budget), format!("{avg_ms:.2} ms"));
                    ui.end_row();
                    ui.label("frame cost (max)");
                    ui.colored_label(cost_color(ui, max_ms, budget), format!("{max_ms:.2} ms"));
                    ui.end_row();
                    ui.label("repaints / sec");
                    ui.label(format!("{ups:.1}"));
                    ui.end_row();
                    if self.working_set > 0 {
                        ui.label("memory (working set)");
                        ui.label(fmt_bytes(self.working_set));
                        ui.end_row();
                        ui.label("memory (private)");
                        ui.label(fmt_bytes(self.private_bytes));
                        ui.end_row();
                    }
                    ui.label("uptime");
                    let s = self.started.elapsed().as_secs();
                    ui.label(format!(
                        "{}h {:02}m {:02}s",
                        s / 3600,
                        (s / 60) % 60,
                        s % 60
                    ));
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Event-driven UI: repaints/sec is demand, not a cap. \
                         Frame cost vs the 16.7 ms (60 fps) budget is what matters.",
                    )
                    .weak()
                    .small(),
                );
            });
        if !open {
            self.visible = false;
        }
        // Keep the numbers fresh without redrawing at full tilt.
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn push(buf: &mut VecDeque<f32>, v: f32) {
    if buf.len() >= WINDOW {
        buf.pop_front();
    }
    buf.push_back(v);
}

fn mean(buf: &VecDeque<f32>) -> f32 {
    if buf.is_empty() {
        0.0
    } else {
        buf.iter().sum::<f32>() / buf.len() as f32
    }
}

fn peak(buf: &VecDeque<f32>) -> f32 {
    buf.iter().copied().fold(0.0, f32::max)
}

fn cost_color(ui: &egui::Ui, ms: f32, budget: f32) -> egui::Color32 {
    if ms > budget {
        ui.visuals().error_fg_color
    } else if ms > budget * 0.5 {
        ui.visuals().warn_fg_color
    } else {
        egui::Color32::from_rgb(140, 200, 140)
    }
}

fn fmt_bytes(b: u64) -> String {
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
fn process_memory() -> (u64, u64) {
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
fn process_memory() -> (u64, u64) {
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
