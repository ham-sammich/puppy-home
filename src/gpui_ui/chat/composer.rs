//! The composer dock: status line + one of four skins (Classic / Unified /
//! Palette / Guided) over the SAME ChatInput entity, the slash-command
//! palette fed by sidecar completions, Agent/Model switcher popovers (the
//! 2.2 popover pattern), and the gear style-preference popover.

use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, ParentElement as _, Styled as _, div, prelude::*,
    px,
};

use crate::backend::CompletionItem;
use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{ChatPop, DashAction, RootView, Tokens};
use crate::session::ComposerStyle;
use crate::workspace::{InstanceStatus, Workspace};

pub struct ComposerArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub input: Entity<ChatInput>,
    pub style: ComposerStyle,
    pub pop: Option<&'a ChatPop>,
    pub puppy: String,
    /// Pending pasted images `(index, thumbnail)` for the chips row.
    pub images: Vec<(usize, std::sync::Arc<gpui::Image>)>,
    /// Completion-palette keyboard selection.
    pub palette_sel: usize,
    /// Dock steer toggle state (false = now, true = queue).
    pub steer_queue: bool,
}

/// The whole bottom dock: palette + status line + skin + footer.
pub fn composer_dock(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .px_4()
        .py_2p5()
        .border_t_1()
        .border_color(t.line_soft)
        .bg(t.panel)
        .child(slash_palette(args))
        .child(status_line(args))
        .children(
            // Unified embeds its chips in-bar; the rest show them above.
            (!args.images.is_empty() && args.style != ComposerStyle::Unified)
                .then(|| image_chips(args)),
        )
        .child(match args.style {
            ComposerStyle::Classic => classic(args),
            ComposerStyle::Unified => unified(args),
            ComposerStyle::Palette => palette(args),
            ComposerStyle::Guided => guided(args),
        })
        .child(footer(args))
        .into_any_element()
}

/// Thumbnails of pending pasted images, each with a remove control.
fn image_chips(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    div()
        .flex()
        .flex_wrap()
        .gap_1p5()
        .children(args.images.iter().map(|(i, img)| {
            let root = args.root.clone();
            let idx = *i;
            div()
                .relative()
                .rounded(px(8.))
                .border_1()
                .border_color(alpha(t.accent, 0.5))
                .overflow_hidden()
                .child(gpui::img(img.clone()).h(px(44.)).max_w(px(96.)))
                .child(
                    div()
                        .id(("img-chip-x", idx as u64))
                        .absolute()
                        .top_0()
                        .right_0()
                        .px_1()
                        .bg(alpha(t.bg, 0.7))
                        .text_size(px(10.))
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.error))
                        .child("\u{2715}")
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::RemoveImage(id, idx), cx)
                            });
                        }),
                )
                .into_any_element()
        }))
        .into_any_element()
}

/// The `@ File` affordance + its directory-browsing popover.
fn at_file_btn(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let open_dir = match args.pop {
        Some(ChatPop::FilePicker(pid, dir)) if *pid == id => Some(dir.clone()),
        _ => None,
    };
    let picker = open_dir.map(|dir| {
        let root_entity = args.root.clone();
        let ws_root = args.ws.root.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        if dir != ws_root {
            let parent = dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let root = args.root.clone();
            rows.push(
                div()
                    .id("picker-up")
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.))
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.well))
                    .child("\u{2191} ..")
                    .on_click(move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::PickerDir(id, parent.clone()), cx)
                        });
                    })
                    .into_any_element(),
            );
        }
        if let Ok(mut entries) = args.ws.fs_handle().read_dir(&dir) {
            entries.sort_by(|a, b| {
                (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase()))
            });
            for (i, entry) in entries
                .into_iter()
                .filter(|e| !e.name.starts_with('.'))
                .take(200)
                .enumerate()
            {
                let root = args.root.clone();
                let path = entry.path.clone();
                let is_dir = entry.is_dir;
                rows.push(
                    div()
                        .id(("picker-row", i as u64))
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.))
                        .font_family("JetBrains Mono")
                        .text_size(px(11.5))
                        .text_color(if is_dir { t.text } else { t.weak })
                        .cursor_pointer()
                        .hover(|d| d.bg(t.well))
                        .child(format!(
                            "{} {}",
                            if is_dir { "\u{25b8}" } else { "\u{b7}" },
                            entry.name
                        ))
                        .on_click(move |_, _, cx| {
                            let action = if is_dir {
                                DashAction::PickerDir(id, path.clone())
                            } else {
                                DashAction::PickerPick(id, path.clone())
                            };
                            root.update(cx, |r, cx| r.dispatch(action, cx));
                        })
                        .into_any_element(),
                );
            }
        }
        let panel = div()
            .occlude()
            .absolute()
            .bottom(px(28.))
            .left_0()
            .w(px(300.))
            .max_h(px(280.))
            .id("picker-scroll")
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .p_1()
            .rounded(px(10.))
            .bg(t.panel)
            .border_1()
            .border_color(t.line_soft)
            .shadow_lg()
            .on_mouse_down_out(move |_, _, cx| {
                root_entity.update(cx, |r, cx| r.dispatch(DashAction::CloseChatPop, cx));
            })
            .children(rows);
        gpui::deferred(panel).with_priority(100)
    });
    div()
        .relative()
        .child(widgets::btn(&t, "@ File").id(("at-file", id.0)).on_click({
            let root = args.root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::PickerOpen(id), cx));
            }
        }))
        .children(picker)
        .into_any_element()
}

/// `. Ready · code-puppy · model` directly above the composer; while a turn
/// runs the right side grows pause/resume + stop + the steer now/queue
/// toggle (B11 — mirrors the egui dock, in every composer style).
fn status_line(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let id = ws.id;
    let color = match ws.status {
        InstanceStatus::Running | InstanceStatus::Thinking | InstanceStatus::ToolCalling => t.run,
        InstanceStatus::WaitingForInput => t.wait,
        InstanceStatus::Paused => t.paused,
        InstanceStatus::Dead => t.error,
        _ => t.weak,
    };
    let mut controls: Vec<AnyElement> = Vec::new();
    if ws.is_running_turn() {
        let mk = |label: &str, key: &'static str, action: DashAction, root: &Entity<RootView>| {
            let root = root.clone();
            widgets::btn(&t, label)
                .id((key, id.0))
                .on_click(move |_, _, cx| {
                    let a = action.clone();
                    root.update(cx, |r, cx| r.dispatch(a, cx));
                })
                .into_any_element()
        };
        if ws.is_paused() {
            controls.push(mk(
                "\u{25b6} Resume",
                "dock-resume",
                DashAction::Resume(id),
                &args.root,
            ));
        } else {
            controls.push(mk(
                "\u{23f8} Pause",
                "dock-pause",
                DashAction::Pause(id),
                &args.root,
            ));
        }
        controls.push(mk(
            "\u{23f9} Stop",
            "dock-stop",
            DashAction::Stop(id),
            &args.root,
        ));
        // Steer delivery toggle: Enter mid-turn steers with this mode.
        let seg = |label: &str, on: bool, queue: bool, idx: u64, root: &Entity<RootView>| {
            let root = root.clone();
            div()
                .id(("dock-steer-mode", id.0 * 2 + idx))
                .px_1p5()
                .py_0p5()
                .rounded(px(6.))
                .text_size(px(10.5))
                .cursor_pointer()
                .when(on, |d| d.bg(alpha(t.accent, 0.18)).text_color(t.accent))
                .when(!on, |d| d.text_color(t.weak))
                .child(label.to_string())
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::SetChatSteerQueue(queue), cx)
                    });
                })
                .into_any_element()
        };
        controls.push(seg(
            "\u{1f3af} now",
            !args.steer_queue,
            false,
            0,
            &args.root,
        ));
        controls.push(seg(
            "\u{1f4e8} queue",
            args.steer_queue,
            true,
            1,
            &args.root,
        ));
    }
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(div().size(px(7.)).rounded_full().bg(color))
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .text_color(t.weak)
                .child(ws.status_line.clone()),
        )
        .child(div().flex_1())
        .children(controls)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The four skins (same input entity, different chrome)
// ---------------------------------------------------------------------------

fn classic(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(input_well(args, t.line_soft))
                .child(send_btn(args, "Send")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(agent_pill(args))
                .child(model_pill(args))
                .child(at_file_btn(args))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(t.dim)
                        .child("Terminal: egui branch only (see GPUI_NOTES.md)"),
                ),
        )
        .into_any_element()
}

fn unified(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .px_2p5()
        .py_1p5()
        .rounded(px(12.))
        .bg(t.well)
        .border_1()
        .border_color(alpha(t.accent, 0.55))
        .children((!args.images.is_empty()).then(|| image_chips(args)))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().min_w_0().flex_1().child(args.input.clone()))
                .child(agent_pill(args))
                .child(model_pill(args))
                .child(at_file_btn(args))
                .child(send_btn(args, "Send")),
        )
        .into_any_element()
}

fn palette(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .rounded(px(10.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .child(
                    div()
                        .text_color(t.accent)
                        .font_weight(FontWeight::BOLD)
                        .child("\u{276f}"),
                )
                .child(div().min_w_0().flex_1().child(args.input.clone()))
                .child(send_btn(args, "\u{21a9}")),
        )
        .child(
            div()
                .flex()
                .gap_3()
                .font_family("JetBrains Mono")
                .text_size(px(10.))
                .text_color(t.dim)
                .child("/ commands")
                .child("@ files")
                .child("\u{21a9} send")
                .child("\u{21e7}\u{21a9} newline"),
        )
        .into_any_element()
}

fn guided(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let starters = [
        "Explain this repo",
        "Fix the failing tests",
        "Review my latest changes",
        "Write tests for the diff",
    ];
    let id = args.ws.id;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1p5()
                .children(starters.iter().enumerate().map(|(i, s)| {
                    let root = args.root.clone();
                    let text = s.to_string();
                    div()
                        .id(("starter", i as u64))
                        .px_2p5()
                        .py_1()
                        .rounded_full()
                        .bg(t.well)
                        .border_1()
                        .border_color(t.line_soft)
                        .text_size(px(11.5))
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|d| d.border_color(alpha(t.accent, 0.6)))
                        .child(*s)
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::StarterPrompt(id, text.clone()), cx)
                            });
                        })
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(labeled(&t, "agent", agent_pill(args)))
                .child(labeled(&t, "model", model_pill(args)))
                .child(input_well(args, t.line_soft))
                .child(send_btn(args, &format!("Send to {} \u{2192}", args.puppy))),
        )
        .into_any_element()
}

fn labeled(t: &Tokens, label: &str, inner: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .text_size(px(9.5))
                .text_color(t.dim)
                .child(label.to_string()),
        )
        .child(inner)
        .into_any_element()
}

fn input_well(args: &ComposerArgs, border: gpui::Rgba) -> AnyElement {
    let t = args.t;
    div()
        .min_w_0()
        .flex_1()
        .px_2p5()
        .py_1p5()
        .rounded(px(10.))
        .bg(t.well)
        .border_1()
        .border_color(border)
        .child(args.input.clone())
        .into_any_element()
}

fn send_btn(args: &ComposerArgs, label: &str) -> AnyElement {
    let root = args.root.clone();
    let id = args.ws.id;
    widgets::primary_btn(&args.t, label)
        .id(("composer-send", id.0))
        .on_click(move |_, _, cx| {
            root.update(cx, |r, cx| r.dispatch(DashAction::ChatSubmit(id), cx));
        })
        .into_any_element()
}

/// Footer: hints left, gear style popover right.
fn footer(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    let gear = div()
        .relative()
        .child(
            div()
                .id("composer-style-gear")
                .px_2()
                .py_0p5()
                .rounded(px(7.))
                .text_size(px(11.))
                .text_color(t.weak)
                .cursor_pointer()
                .hover(|d| d.text_color(t.text).bg(t.well))
                .child(format!("\u{2699} Composer: {}", args.style.label()))
                .on_click({
                    let root = root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::ToggleChatPop(ChatPop::Style), cx)
                        });
                    }
                }),
        )
        .children(matches!(args.pop, Some(ChatPop::Style)).then(|| {
            pop_panel(
                &t,
                &root,
                ComposerStyle::ALL
                    .iter()
                    .map(|s| {
                        (
                            s.label().to_string(),
                            s.description().to_string(),
                            *s == args.style,
                            DashAction::SetComposerStyle(*s),
                        )
                    })
                    .collect(),
            )
        }));
    div()
        .flex()
        .items_center()
        .child(
            div()
                .text_size(px(10.5))
                .text_color(t.dim)
                .child("\u{23ce} send \u{b7} \u{21e7}\u{23ce} newline"),
        )
        .child(div().flex_1())
        .child(gear)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Switchers + palette
// ---------------------------------------------------------------------------

fn agent_pill(args: &ComposerArgs) -> AnyElement {
    let ws = args.ws;
    let id = ws.id;
    let label = if ws.agent.is_empty() {
        "agent\u{2026}".to_string()
    } else {
        ws.agent.clone()
    };
    let open = matches!(args.pop, Some(ChatPop::Agent(p)) if *p == id);
    let items = open.then(|| {
        ws.agent_catalog()
            .iter()
            .map(|a| {
                let name = if a.display_name.is_empty() {
                    a.name.clone()
                } else {
                    a.display_name.clone()
                };
                (
                    name,
                    a.description.clone(),
                    a.current,
                    DashAction::SetAgent(id, a.name.clone()),
                )
            })
            .collect()
    });
    switch_pill(args, "agent-pill", label, ChatPop::Agent(id), items)
}

fn model_pill(args: &ComposerArgs) -> AnyElement {
    let ws = args.ws;
    let id = ws.id;
    let label = if ws.model.is_empty() {
        "model\u{2026}".to_string()
    } else {
        ws.model.clone()
    };
    let open = matches!(args.pop, Some(ChatPop::Model(p)) if *p == id);
    let items = open.then(|| {
        ws.model_catalog()
            .iter()
            .map(|m| {
                (
                    m.name.clone(),
                    m.description.clone(),
                    m.name == ws.model,
                    DashAction::SetModel(id, m.name.clone()),
                )
            })
            .collect()
    });
    switch_pill(args, "model-pill", label, ChatPop::Model(id), items)
}

/// A mono pill that toggles a popover; popover items dispatch actions.
fn switch_pill(
    args: &ComposerArgs,
    key: &'static str,
    label: String,
    pop: ChatPop,
    items: Option<Vec<(String, String, bool, DashAction)>>,
) -> AnyElement {
    let t = args.t;
    let root = args.root.clone();
    div()
        .relative()
        .child(
            div()
                .id((key, args.ws.id.0))
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .text_color(t.weak)
                .max_w(px(180.))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|d| d.border_color(alpha(t.accent, 0.6)).text_color(t.text))
                .child(label)
                .on_click({
                    let root = root.clone();
                    move |_, _, cx| {
                        root.update(cx, |r, cx| {
                            r.dispatch(DashAction::ToggleChatPop(pop.clone()), cx)
                        });
                    }
                }),
        )
        .children(items.map(|list| pop_panel(&t, &root, list)))
        .into_any_element()
}

/// Shared popover panel: anchored above the pill, deferred over everything.
/// `(label, description, selected, action)` per row.
fn pop_panel(
    t: &Tokens,
    root: &Entity<RootView>,
    items: Vec<(String, String, bool, DashAction)>,
) -> AnyElement {
    let root = root.clone();
    let panel = div()
        .occlude()
        .absolute()
        .bottom(px(28.))
        .left_0()
        .min_w(px(240.))
        .max_h(px(300.))
        .id("chat-pop-scroll")
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_1()
        .rounded(px(10.))
        .bg(t.panel)
        .border_1()
        .border_color(t.line_soft)
        .shadow_lg()
        .on_mouse_down_out({
            let root = root.clone();
            move |_, _, cx| {
                root.update(cx, |r, cx| r.dispatch(DashAction::CloseChatPop, cx));
            }
        })
        .children(if items.is_empty() {
            vec![
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(t.weak)
                    .child("catalog not loaded yet")
                    .into_any_element(),
            ]
        } else {
            items
                .into_iter()
                .enumerate()
                .map(|(i, (label, desc, sel, action))| {
                    let root = root.clone();
                    div()
                        .id(("chat-pop-opt", i as u64))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(px(7.))
                        .cursor_pointer()
                        .when(sel, |d| d.bg(alpha(t.accent, 0.12)))
                        .hover(|d| d.bg(t.well))
                        .child(div().text_size(px(11.5)).text_color(t.text).child(label))
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(t.dim)
                                .max_w(px(150.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(desc),
                        )
                        .when(sel, |d| {
                            d.child(div().text_color(t.accent).child("\u{2713}"))
                        })
                        .on_click(move |_, _, cx| {
                            let a = action.clone();
                            root.update(cx, |r, cx| {
                                r.dispatch(a, cx);
                                r.dispatch(DashAction::CloseChatPop, cx);
                            });
                        })
                        .into_any_element()
                })
                .collect()
        });
    gpui::deferred(panel).with_priority(100).into_any_element()
}

/// Sidecar-completion palette shown above the composer while typing `/`/`@`.
fn slash_palette(args: &ComposerArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    if !ws.completions_open() {
        return div().into_any_element();
    }
    let id = ws.id;
    let root = args.root.clone();
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_1()
        .rounded(px(10.))
        .bg(t.card)
        .border_1()
        .border_color(alpha(t.accent, 0.4))
        .max_h(px(220.))
        .id(("slash-palette", id.0))
        .overflow_y_scroll()
        .child(
            div()
                .px_2()
                .py_0p5()
                .text_size(px(9.5))
                .text_color(t.dim)
                .child("completions \u{b7} \u{2191}\u{2193} navigate \u{b7} \u{23ce} insert \u{b7} esc dismiss"),
        )
        .children(
            ws.completion_items()
                .iter()
                .take(30)
                .enumerate()
                .map(|(i, item)| {
                    let root = root.clone();
                    let shown = display_of(item);
                    let selected = i == args.palette_sel;
                    div()
                        .id(("comp-opt", i as u64))
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.))
                        .font_family("JetBrains Mono")
                        .text_size(px(11.5))
                        .text_color(t.text)
                        .cursor_pointer()
                        .when(selected, |d| d.bg(alpha(t.accent, 0.16)))
                        .hover(|d| d.bg(t.well))
                        .child(shown)
                        .on_click(move |_, _, cx| {
                            root.update(cx, |r, cx| {
                                r.dispatch(DashAction::ApplyCompletion(id, i), cx)
                            });
                        })
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn display_of(item: &CompletionItem) -> String {
    if item.display.is_empty() {
        item.text.clone()
    } else {
        item.display.clone()
    }
}

/// Apply a sidecar completion to the input text (caret assumed at the end —
/// the GPUI composer requests completions for the full text, like egui).
/// `start_position` is a char offset <= 0 relative to the caret.
pub fn apply_completion(text: &str, item: &CompletionItem) -> String {
    let chars = text.chars().count() as i64;
    let start_chars = (chars + item.start_position.min(0)).max(0) as usize;
    let byte = text
        .char_indices()
        .nth(start_chars)
        .map(|(b, _)| b)
        .unwrap_or(text.len());
    format!("{}{}", &text[..byte], item.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, start: i64) -> CompletionItem {
        serde_json::from_value(serde_json::json!({
            "text": text,
            "start_position": start,
        }))
        .expect("valid completion item")
    }

    #[test]
    fn completion_replaces_tail_chars() {
        // "/mo" + completion "model" replacing the last 2 chars -> "/model"
        assert_eq!(apply_completion("/mo", &item("model", -2)), "/model");
        // start 0 = pure insert at caret.
        assert_eq!(apply_completion("/cd ", &item("src/", 0)), "/cd src/");
        // Over-long negative start clamps to the whole string.
        assert_eq!(apply_completion("/x", &item("/help", -99)), "/help");
    }
}
