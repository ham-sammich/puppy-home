//! The Models manager (QW4): the sidecar's model catalog + the user's
//! `extra_models.json` overlay, joining the MCP/Skills/Agents trio.
//!
//! Catalog rows come from the serving workspace's announced model list
//! (name / description / current — the same feed the chat model picker
//! uses). Local metadata (provider `type`, `context_length`, custom
//! badge) is joined from code_puppy's data files: `extra_models.json`
//! (user-authored overlay, editable here) and the OAuth model files
//! (read-only). Set-active rides the existing `set_model` op on the
//! serving workspace.
//!
//! Editing is the whole-file JSON paste pattern (syntect-highlighted,
//! like the MCP/agent wizards' paste mode) rather than per-field forms:
//! extra_models.json entries are free-form provider configs — a form
//! would lie about the schema. New/changed models need a workspace
//! restart to load (the sidecar reads model files at spawn).

use std::path::PathBuf;

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::RootView;
use crate::gpui_ui::managers::MgrAction;
use crate::gpui_ui::managers_ui::{MgrArgs, act, center_hint, filter_field, paste_panel, small};
use crate::gpui_ui::widgets::{self, alpha};
use crate::workspace::Workspace;

/// code_puppy's data dir: `$XDG_DATA_HOME/code_puppy` when the env var is
/// set, else the legacy `~/.code_puppy` (mirrors config.py `_get_xdg_dir`).
pub(crate) fn data_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return Some(p.join("code_puppy"));
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".code_puppy"))
}

fn extra_models_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("extra_models.json"))
}

/// Best-effort parse of one models file into (name -> entry).
fn load_models_file(path: &PathBuf) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Display metadata for one catalog model, joined from the local files.
pub(crate) struct ModelMeta {
    pub provider: Option<String>,
    pub context_length: Option<u64>,
    /// Present in the user's extra_models.json (editable/removable here).
    pub custom: bool,
}

/// Join metadata for `name` from extra_models.json + the OAuth files.
/// API keys and endpoints in those files are never surfaced here — only
/// `type` and `context_length`.
pub(crate) fn model_meta(name: &str) -> ModelMeta {
    let mut meta = ModelMeta {
        provider: None,
        context_length: None,
        custom: false,
    };
    let Some(dir) = data_dir() else { return meta };
    let sources = [
        ("extra_models.json", true),
        ("claude_models.json", false),
        ("chatgpt_models.json", false),
        ("gemini_models.json", false),
        ("copilot_models.json", false),
    ];
    for (file, is_extra) in sources {
        let map = load_models_file(&dir.join(file));
        if let Some(entry) = map.get(name) {
            meta.custom |= is_extra;
            if meta.provider.is_none() {
                meta.provider = entry
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            if meta.context_length.is_none() {
                meta.context_length = entry.get("context_length").and_then(|v| v.as_u64());
            }
        }
    }
    meta
}

/// Current extra_models.json text (pretty), `{}` template when absent.
pub(crate) fn extra_models_text() -> String {
    extra_models_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| "{}".to_string())
}

impl RootView {
    pub(crate) fn dispatch_models(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        match action {
            MgrAction::ModelSetActive(name) => {
                if let Some(id) = self.first_ready_ws()
                    && let Some(ws) = self.supervisor.get_mut(id)
                {
                    ws.set_model_live(&name);
                }
                self.toast(
                    format!("Switching model to {name}\u{2026}"),
                    self.tokens.run,
                );
            }
            MgrAction::ModelsEditorOpen => {
                self.models_editor = true;
                self.seed_paste(extra_models_text(), cx);
            }
            MgrAction::ModelsEditorCancel => self.models_editor = false,
            MgrAction::ModelsEditorSave => {
                let text = self.paste_text(cx);
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) if v.is_object() => {
                        match extra_models_path()
                            .ok_or_else(|| "no data dir".to_string())
                            .and_then(|p| {
                                std::fs::write(&p, text.as_bytes()).map_err(|e| e.to_string())
                            }) {
                            Ok(()) => {
                                self.models_editor = false;
                                self.toast(
                                    "extra_models.json saved \u{2014} restart workspaces to load"
                                        .to_string(),
                                    self.tokens.run,
                                );
                            }
                            Err(e) => self.last_error = Some(format!("save failed: {e}")),
                        }
                    }
                    Ok(_) => {
                        self.last_error =
                            Some("extra_models.json must be a JSON object".to_string())
                    }
                    Err(e) => self.last_error = Some(format!("invalid JSON: {e}")),
                }
            }
            MgrAction::ModelRemove(name) => {
                let Some(path) = extra_models_path() else {
                    return;
                };
                let mut map = load_models_file(&path);
                if map.remove(&name).is_some() {
                    let pretty = serde_json::to_string_pretty(&serde_json::Value::Object(map))
                        .unwrap_or_else(|_| "{}".to_string());
                    match std::fs::write(&path, pretty.as_bytes()) {
                        Ok(()) => self.toast(
                            format!("{name} removed \u{2014} restart workspaces to apply"),
                            self.tokens.wait,
                        ),
                        Err(e) => self.last_error = Some(format!("save failed: {e}")),
                    }
                }
            }
            _ => {}
        }
    }
}

/// The Models list (or the extra_models.json editor when open).
pub(crate) fn body(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    if args.models_editor {
        return editor_body(args);
    }
    let filter = args.filter.to_lowercase();
    let catalog = ws.model_catalog();
    if catalog.is_empty() {
        return center_hint(
            &t,
            &[
                "No model catalog yet",
                "The sidecar announces models shortly after it's ready.",
            ],
        );
    }
    let rows: Vec<AnyElement> = catalog
        .iter()
        .filter(|m| filter.is_empty() || m.name.to_lowercase().contains(&filter))
        .map(|m| {
            let meta = model_meta(&m.name);
            let mut badges = div()
                .flex()
                .items_center()
                .gap_1p5()
                .children(meta.provider.as_ref().map(|p| small(&t, p.clone(), t.weak)))
                .children(
                    meta.context_length
                        .map(|c| small(&t, format!("{}k ctx", c / 1000), t.dim)),
                );
            if meta.custom {
                badges = badges.child(
                    div()
                        .px_1p5()
                        .rounded(px(6.))
                        .bg(alpha(t.wait, 0.15))
                        .text_size(px(10.))
                        .text_color(t.wait)
                        .child("custom"),
                );
            }
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(if m.current { t.run } else { t.line_soft })
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(if m.current { t.run } else { t.text })
                        .child(if m.current {
                            format!("\u{25cf} {}", m.name)
                        } else {
                            m.name.clone()
                        }),
                )
                .child(badges)
                .child(div().flex_1())
                .children((!m.current).then(|| {
                    widgets::btn(&t, "Set active")
                        .id(gpui::SharedString::from(format!("model-set-{}", m.name)))
                        .on_click(act(&args.root, MgrAction::ModelSetActive(m.name.clone())))
                }))
                .children((meta.custom).then(|| {
                    widgets::btn(&t, "\u{2715}")
                        .id(gpui::SharedString::from(format!("model-rm-{}", m.name)))
                        .on_click(act(&args.root, MgrAction::ModelRemove(m.name.clone())))
                }))
                .into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .gap_2()
        .size_full()
        .child(filter_field(args, "filter models\u{2026}"))
        .child(
            div()
                .id("models-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1p5()
                .children(rows),
        )
        .into_any_element()
}

/// Whole-file JSON editor over extra_models.json.
fn editor_body(args: &MgrArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .size_full()
        .child(small(
            &t,
            "extra_models.json \u{2014} user model overlay (JSON object: name \u{2192} provider config). \
             Saved models load on the NEXT workspace spawn.",
            t.weak,
        ))
        .child(paste_panel(args))
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    widgets::primary_btn(&t, "Save")
                        .id("models-save")
                        .on_click(act(&args.root, MgrAction::ModelsEditorSave)),
                )
                .child(
                    widgets::btn(&t, "Cancel")
                        .id("models-cancel")
                        .on_click(act(&args.root, MgrAction::ModelsEditorCancel)),
                ),
        )
        .into_any_element()
}
