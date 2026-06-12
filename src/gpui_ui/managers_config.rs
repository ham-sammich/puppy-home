//! The puppy.cfg manager (QW5): code_puppy's global config surfaced as
//! an editable settings list.
//!
//! The file is INI (`[puppy]` section, `configparser`-written). Editing
//! is LINE-LEVEL — we rewrite only the `key = value` line being changed
//! (or append to the `[puppy]` section), so comments, unknown sections,
//! and ordering all survive round-trips that Python's configparser
//! itself wouldn't preserve.
//!
//! Identity + secrets care: values for keys that look secret
//! (key/token/secret/password) are shown masked and are NOT editable
//! here — edit the file directly for those. Values are never logged.
//! Most settings only take effect in NEW sidecars ("restart workspaces
//! to apply" shown in the header).

use std::path::PathBuf;

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, prelude::*, px};

use crate::gpui_ui::RootView;
use crate::gpui_ui::managers::{F_NAME, MgrAction};
use crate::gpui_ui::managers_ui::{MgrArgs, act, center_hint, field, small};
use crate::gpui_ui::widgets;

/// Keys pinned to the top of the list, in display order. Everything else
/// (banner colors etc.) follows alphabetically.
const PRIORITY: &[&str] = &[
    "puppy_name",
    "owner_name",
    "model",
    "default_agent",
    "yolo_mode",
    "allow_recursion",
    "auto_save_session",
    "max_saved_sessions",
    "compaction_strategy",
    "compaction_threshold",
    "protected_token_count",
    "message_limit",
    "temperature",
];

/// config dir: `$XDG_CONFIG_HOME/code_puppy` when set, else `~/.code_puppy`
/// (mirrors config.py `_get_xdg_dir`).
fn cfg_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return Some(p.join("code_puppy").join("puppy.cfg"));
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".code_puppy").join("puppy.cfg"))
}

/// Should this key's value be masked + locked? (identity files carry
/// tokens; never display or edit them here).
pub(crate) fn secret_key(key: &str) -> bool {
    let k = key.to_lowercase();
    ["key", "token", "secret", "password"]
        .iter()
        .any(|s| k.contains(s))
}

/// Parse the `[puppy]` section into ordered (key, value) pairs.
pub(crate) fn parse_puppy_section(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_section = l == "[puppy]";
            continue;
        }
        if !in_section || l.is_empty() || l.starts_with('#') || l.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

/// Rewrite ONE key's line inside `[puppy]` (append at section end when
/// absent), preserving everything else byte-for-byte.
pub(crate) fn set_value_in(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut in_section = false;
    let mut section_end = lines.len(); // insertion point when key absent
    for (i, line) in lines.iter().enumerate() {
        let l = line.trim();
        if l.starts_with('[') {
            if in_section {
                section_end = i;
                break;
            }
            in_section = l == "[puppy]";
            continue;
        }
        if in_section {
            if let Some((k, _)) = l.split_once('=')
                && k.trim() == key
            {
                lines[i] = format!("{key} = {value}");
                return lines.join("\n") + "\n";
            }
            if !l.is_empty() {
                section_end = i + 1;
            }
        }
    }
    lines.insert(section_end, format!("{key} = {value}"));
    lines.join("\n") + "\n"
}

/// Priority keys first (fixed order), the rest alphabetical.
pub(crate) fn sort_for_display(mut entries: Vec<(String, String)>) -> Vec<(String, String)> {
    entries.sort_by_key(|(k, _)| match PRIORITY.iter().position(|p| p == k) {
        Some(i) => (0, i, String::new()),
        None => (1, 0, k.clone()),
    });
    entries
}

impl RootView {
    /// (Re)load puppy.cfg into the manager cache.
    pub(crate) fn cfg_reload(&mut self) {
        self.cfg_entries = cfg_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| sort_for_display(parse_puppy_section(&t)))
            .unwrap_or_default();
    }

    pub(crate) fn dispatch_config(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        match action {
            MgrAction::CfgEdit(key) => {
                if secret_key(&key) {
                    return;
                }
                let current = self
                    .cfg_entries
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                self.seed(F_NAME, current, cx);
                self.cfg_edit_key = Some(key);
            }
            MgrAction::CfgEditCancel => self.cfg_edit_key = None,
            MgrAction::CfgEditSave => {
                let Some(key) = self.cfg_edit_key.take() else {
                    return;
                };
                let value = self.mgr_input_text(F_NAME, cx);
                let Some(path) = cfg_path() else { return };
                let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "[puppy]".into());
                let new = set_value_in(&text, &key, value.trim());
                match std::fs::write(&path, new.as_bytes()) {
                    Ok(()) => {
                        self.cfg_reload();
                        self.toast(
                            format!("{key} saved \u{2014} restart workspaces to apply"),
                            self.tokens.run,
                        );
                    }
                    Err(e) => self.last_error = Some(format!("save failed: {e}")),
                }
            }
            _ => {}
        }
    }
}

/// The settings list (one row editable at a time via the shared input).
pub(crate) fn body(args: &MgrArgs) -> AnyElement {
    let t = args.t;
    if args.cfg_entries.is_empty() {
        return center_hint(
            &t,
            &[
                "No puppy.cfg found",
                "Run Code Puppy once to create it, then Refresh.",
            ],
        );
    }
    let filter = args.filter.to_lowercase();
    let rows: Vec<AnyElement> = args
        .cfg_entries
        .iter()
        .filter(|(k, _)| filter.is_empty() || k.to_lowercase().contains(&filter))
        .map(|(k, v)| {
            let editing = args.cfg_edit_key == Some(k.as_str());
            let secret = secret_key(k);
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(if editing { t.run } else { t.line_soft })
                .child(
                    div()
                        .w(px(220.))
                        .text_size(px(11.5))
                        .font_family("JetBrains Mono")
                        .text_color(t.text)
                        .child(k.clone()),
                );
            if editing {
                row = row
                    .child(
                        div()
                            .flex_1()
                            .child(field(&t, "", args.inputs.get(F_NAME), false)),
                    )
                    .child(
                        widgets::primary_btn(&t, "Save")
                            .id(gpui::SharedString::from(format!("cfg-save-{k}")))
                            .on_click(act(&args.root, MgrAction::CfgEditSave)),
                    )
                    .child(
                        widgets::btn(&t, "Cancel")
                            .id(gpui::SharedString::from(format!("cfg-cancel-{k}")))
                            .on_click(act(&args.root, MgrAction::CfgEditCancel)),
                    );
            } else {
                row = row
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(if secret { t.dim } else { t.weak })
                            .child(if secret {
                                "\u{25cf}\u{25cf}\u{25cf}\u{25cf}\u{25cf} (hidden \u{2014} edit the file directly)"
                                    .to_string()
                            } else if v.is_empty() {
                                "(empty)".to_string()
                            } else {
                                v.clone()
                            }),
                    )
                    .children((!secret).then(|| {
                        widgets::btn(&t, "Edit")
                            .id(gpui::SharedString::from(format!("cfg-edit-{k}")))
                            .on_click(act(&args.root, MgrAction::CfgEdit(k.clone())))
                    }));
            }
            row.into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .gap_2()
        .size_full()
        .child(small(
            &t,
            "Most settings apply to NEW sidecars \u{2014} restart workspaces after editing.",
            t.weak,
        ))
        .child(crate::gpui_ui::managers_ui::filter_field(
            args,
            "filter settings\u{2026}",
        ))
        .child(
            div()
                .id("cfg-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .children(rows),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "# comment kept\n[puppy]\npuppy_name = Rex\nmodel = gpt-5\n\n[other]\nkeep = me\n";

    #[test]
    fn parse_reads_only_the_puppy_section() {
        let e = parse_puppy_section(SAMPLE);
        assert_eq!(
            e,
            vec![
                ("puppy_name".into(), "Rex".into()),
                ("model".into(), "gpt-5".into())
            ]
        );
    }

    #[test]
    fn set_value_replaces_in_place_preserving_everything_else() {
        let out = set_value_in(SAMPLE, "model", "claude");
        assert!(out.contains("# comment kept"));
        assert!(out.contains("model = claude"));
        assert!(out.contains("[other]\nkeep = me"));
        assert!(!out.contains("gpt-5"));
    }

    #[test]
    fn set_value_appends_missing_key_inside_the_section() {
        let out = set_value_in(SAMPLE, "yolo_mode", "true");
        let puppy_ix = out.find("[puppy]").unwrap();
        let other_ix = out.find("[other]").unwrap();
        let key_ix = out.find("yolo_mode = true").unwrap();
        assert!(puppy_ix < key_ix && key_ix < other_ix);
    }

    #[test]
    fn secret_keys_detected() {
        assert!(secret_key("openai_api_key"));
        assert!(secret_key("MY_TOKEN"));
        assert!(!secret_key("puppy_name"));
    }

    #[test]
    fn priority_keys_sort_first() {
        let sorted = sort_for_display(vec![
            ("banner_color_grep".into(), "x".into()),
            ("model".into(), "m".into()),
            ("puppy_name".into(), "p".into()),
        ]);
        assert_eq!(sorted[0].0, "puppy_name");
        assert_eq!(sorted[1].0, "model");
        assert_eq!(sorted[2].0, "banner_color_grep");
    }
}
