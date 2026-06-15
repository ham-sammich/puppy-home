//! About / version + updates (QW1): the toolbar version chip and its
//! panel — installed code_puppy version (from the sidecar `ready`
//! announcement), a PyPI "check for updates", and the honest update
//! action.
//!
//! WHAT "UPDATE" REALLY MEANS HERE: sidecars launch via
//! `uv run --with code-puppy python sidecar.py` (backend::spawn), which
//! resolves code-puppy from uv's environment cache. The Update action
//! runs `uv run --refresh-package code-puppy --with code-puppy python -c
//! "import code_puppy; print(code_puppy.__version__)"` — forcing uv to
//! re-resolve + fetch the latest release into its cache and reporting
//! the version it landed on. Running workspaces keep their old code;
//! NEW spawns pick up the refreshed version, hence the "restart
//! workspaces to apply" messaging.
//!
//! The PyPI check shells out to `curl` (10s timeout) rather than adding
//! an HTTP dependency for one JSON fetch; offline/no-curl degrades to an
//! inline error message.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use gpui::prelude::*;
use gpui::{Entity, FontWeight, IntoElement, div, px};

use crate::gpui_ui::widgets;
use crate::gpui_ui::{DashAction, RootView, Tokens};

/// Panel interactions, nested under `DashAction::About`.
#[derive(Clone, Copy, Debug)]
pub enum AboutAction {
    Toggle,
    Check,
    Update,
}

/// Worker results delivered through the drain loop.
pub enum AboutMsg {
    /// PyPI's latest published version (or why we couldn't fetch it).
    Latest(Result<String, String>),
    /// The version uv's refreshed cache resolved to (or the failure).
    Updated(Result<String, String>),
}

#[derive(Default)]
pub struct AboutState {
    pub open: bool,
    pub checking: bool,
    pub updating: bool,
    /// Outcome of the last PyPI check.
    pub latest: Option<Result<String, String>>,
    /// Outcome of the last update run.
    pub update_result: Option<Result<String, String>>,
    rx: Option<Receiver<AboutMsg>>,
}

impl AboutState {
    /// Fold any finished worker results into the state (drain-side).
    pub fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                AboutMsg::Latest(r) => {
                    self.checking = false;
                    self.latest = Some(r);
                }
                AboutMsg::Updated(r) => {
                    self.updating = false;
                    self.update_result = Some(r);
                }
            }
        }
    }

    fn channel(&mut self) -> Sender<AboutMsg> {
        // One receiver serves both workers; recreate only when absent.
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        tx
    }

    /// Kick the PyPI latest-version fetch off-thread.
    pub fn check(&mut self, waker: Arc<dyn crate::waker::UiWaker>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.latest = None;
        let tx = self.channel();
        std::thread::spawn(move || {
            let _ = tx.send(AboutMsg::Latest(fetch_latest()));
            waker.wake();
        });
    }

    /// Kick the uv cache refresh off-thread.
    pub fn update(&mut self, waker: Arc<dyn crate::waker::UiWaker>) {
        if self.updating {
            return;
        }
        self.updating = true;
        self.update_result = None;
        let tx = self.channel();
        std::thread::spawn(move || {
            let _ = tx.send(AboutMsg::Updated(run_update()));
            waker.wake();
        });
    }
}

/// `curl https://pypi.org/pypi/code-puppy/json` → `info.version`.
fn fetch_latest() -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            "https://pypi.org/pypi/code-puppy/json",
        ])
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !out.status.success() {
        return Err("PyPI unreachable (offline?)".to_string());
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad PyPI response: {e}"))?;
    v.get("info")
        .and_then(|i| i.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "PyPI response missing info.version".to_string())
}

/// Refresh uv's cached code-puppy resolution and report what it landed on.
/// Bounded: a wedged network can stall `uv` indefinitely and
/// `Command::output` would pin the worker thread (and the "updating"
/// spinner) forever — so we poll `try_wait` and kill at the deadline
/// (G1 audit fix).
fn run_update() -> Result<String, String> {
    use std::io::Read as _;
    let mut child = std::process::Command::new("uv")
        .args([
            "run",
            "--refresh-package",
            "code-puppy",
            "--with",
            "code-puppy",
            "python",
            "-c",
            "import code_puppy; print(code_puppy.__version__)",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run uv: {e}"))?;
    // Drain both pipes on threads (uv's progress chatter can exceed the
    // 64KB pipe buffer — polling without reading would deadlock us into
    // the timeout path every time).
    let read_thread = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        })
    };
    let out_t = read_thread(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    );
    let err_t = read_thread(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("uv refresh timed out after 5 minutes".to_string());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(250)),
            Err(e) => return Err(format!("uv wait failed: {e}")),
        }
    };
    let out = std::process::Output {
        status,
        stdout: out_t.join().unwrap_or_default().into_bytes(),
        stderr: err_t.join().unwrap_or_default().into_bytes(),
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "uv refresh failed: {}",
            err.lines().last().unwrap_or("unknown error")
        ));
    }
    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ver.is_empty() {
        return Err("uv refresh produced no version".to_string());
    }
    Ok(ver)
}

/// Naive-but-honest version ordering: split on '.', compare numerically,
/// non-numeric segments compare as strings.
pub fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<(u64, String)> {
        s.split('.')
            .map(|p| (p.parse::<u64>().unwrap_or(0), p.to_string()))
            .collect()
    };
    let (l, c) = (parse(latest), parse(current));
    for i in 0..l.len().max(c.len()) {
        let a = l.get(i);
        let b = c.get(i);
        match (a, b) {
            (Some(x), Some(y)) => {
                if x.0 != y.0 {
                    return x.0 > y.0;
                }
                if x.1 != y.1 {
                    return x.1 > y.1;
                }
            }
            (Some(_), None) => return true,
            (None, _) => return false,
        }
    }
    false
}

/// The floating About panel (anchored under the toolbar, top-right).
pub fn panel(
    t: &Tokens,
    st: &AboutState,
    current: String,
    root: &Entity<RootView>,
) -> impl IntoElement {
    let current = current.as_str();
    let act = |root: &Entity<RootView>, a: AboutAction| {
        let root = root.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App| {
            root.update(cx, |r, cx| r.dispatch(DashAction::About(a), cx));
        }
    };

    // Two DISTINCT versions, each explicitly labeled so they can't be
    // confused (the toolbar chip used to show the code_puppy version with
    // no label, reading like it was the app's own — P3).
    let ver_line = |label: &str, sub: &str, ver: String| {
        div()
            .flex()
            .items_baseline()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(t.weak)
                    .child(sub.to_string()),
            )
            .child(div().text_color(t.weak).child(ver))
    };
    let mut body = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(ver_line(
            "Doghouse",
            "(this app)",
            format!("v{}", crate::plugin::HOST_VERSION),
        ))
        .child(ver_line(
            "code_puppy",
            "(agent engine)",
            format!(
                "installed: {}",
                if current.is_empty() { "?" } else { current }
            ),
        ))
        .child(div().text_size(px(11.)).text_color(t.weak).child(
            "Sidecars resolve code-puppy through uv at spawn; updating \
             refreshes uv's cache, then restart workspaces to apply.",
        ));

    // Latest-version line (after a check).
    match &st.latest {
        Some(Ok(latest)) => {
            let newer = version_newer(latest, current);
            body =
                body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(if newer { t.wait } else { t.run }).child(
                            if newer {
                                format!("PyPI has {latest} — update available")
                            } else {
                                format!("PyPI: {latest} — you're current")
                            },
                        ))
                        .when(newer && !st.updating, |d| {
                            d.child(
                                widgets::btn(t, "Update now")
                                    .id("about-update")
                                    .on_click(act(root, AboutAction::Update)),
                            )
                        }),
                );
        }
        Some(Err(e)) => {
            body = body.child(
                div()
                    .text_color(t.error)
                    .child(format!("Check failed: {e}")),
            );
        }
        None => {}
    }

    // Update outcome line.
    match &st.update_result {
        Some(Ok(v)) => {
            body = body.child(div().text_color(t.run).child(format!(
                "uv cache refreshed to {v} — restart workspaces to apply"
            )));
        }
        Some(Err(e)) => {
            body = body.child(
                div()
                    .text_color(t.error)
                    .child(format!("Update failed: {e}")),
            );
        }
        None => {}
    }

    body = body.child(
        div()
            .flex()
            .gap_2()
            .child(if st.checking {
                div()
                    .text_color(t.weak)
                    .child("Checking PyPI\u{2026}")
                    .into_any_element()
            } else {
                widgets::btn(t, "Check for updates")
                    .id("about-check")
                    .on_click(act(root, AboutAction::Check))
                    .into_any_element()
            })
            .when(st.updating, |d| {
                d.child(
                    div()
                        .text_color(t.weak)
                        .child("Refreshing uv cache\u{2026}"),
                )
            }),
    );

    div()
        .absolute()
        .top(px(44.))
        .right(px(12.))
        .w(px(380.))
        .p_3()
        .rounded(px(10.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .text_size(px(12.5))
        .child(body)
        .child(
            div().mt_2().child(
                widgets::btn(t, "Close")
                    .id("about-close")
                    .on_click(act(root, AboutAction::Toggle)),
            ),
        )
        .occlude()
}

#[cfg(test)]
mod tests {
    use super::version_newer;

    #[test]
    fn version_ordering() {
        assert!(version_newer("1.2.10", "1.2.9"));
        assert!(version_newer("2.0", "1.9.9"));
        assert!(!version_newer("1.2.9", "1.2.9"));
        assert!(!version_newer("1.2.8", "1.2.9"));
        assert!(version_newer("1.2.9.1", "1.2.9"));
    }
}
