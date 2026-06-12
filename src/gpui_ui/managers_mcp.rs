//! MCP manager: dispatch + render (egui `views/mcp_manager.rs` +
//! `views/mcp_wizard/` at parity, dressed in the GPUI tokens).

use gpui::{AnyElement, IntoElement, ParentElement as _, Rgba, Styled as _, div, prelude::*, px};

use crate::gpui_ui::managers::{
    F_B, F_C, F_D, F_E, F_F, F_NAME, MgrAction, lines_to_pairs, pairs_to_lines,
};
use crate::gpui_ui::managers_ui::{
    MgrArgs, act, center_hint, field, mode_toggle, mono_block, option_card, paste_panel, small,
    step_strip, switch,
};
use crate::gpui_ui::widgets;
use crate::gpui_ui::{RootView, Tokens};
use crate::views::common::EditMode;
use crate::views::mcp_wizard::{self, Transport};
use crate::workspace::Workspace;

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl RootView {
    pub(crate) fn dispatch_mcp(&mut self, action: MgrAction, cx: &mut gpui::Context<Self>) {
        let accent = self.tokens.accent;
        match action {
            MgrAction::McpToggle(name, on) => {
                // Optimistic: the pending value wins until fresh data lands.
                self.mgr_pending.insert(name.clone(), on);
                self.with_ready_backend(|b| b.set_mcp_enabled(&name, on));
            }
            MgrAction::McpWizardOpen => {
                self.ensure_mgr_inputs(cx);
                let w = mcp_wizard::Wizard::new();
                self.seed_mcp_inputs(&w, cx);
                self.mcp_wizard = Some(w);
            }
            MgrAction::McpWizardCancel => self.mcp_wizard = None,
            MgrAction::McpTransport(ix) => {
                if let Some(w) = &mut self.mcp_wizard {
                    w.transport = match ix {
                        1 => Transport::Sse,
                        2 => Transport::Http,
                        _ => Transport::Stdio,
                    };
                }
            }
            MgrAction::McpMode(paste) => {
                self.read_mcp_inputs(cx);
                let mut seed = None;
                if let Some(w) = &mut self.mcp_wizard {
                    w.error = None;
                    w.mode = if paste {
                        EditMode::Paste
                    } else {
                        EditMode::Form
                    };
                    if paste {
                        // egui semantics: entering Paste re-seeds from the form.
                        w.sync_paste_from_form();
                        seed = Some(w.paste.clone());
                    }
                }
                if let Some(text) = seed {
                    self.seed_paste(text, cx);
                }
            }
            MgrAction::McpStep(delta) => {
                self.read_mcp_inputs(cx);
                if let Some(w) = &mut self.mcp_wizard {
                    w.error = None;
                    let next = (w.step as i32 + delta).clamp(0, 2) as usize;
                    // egui gates Details -> Review behind field validation.
                    if next == 2
                        && delta > 0
                        && let Err(e) = mcp_wizard::validate_fields(w)
                    {
                        w.error = Some(e);
                        return;
                    }
                    w.step = next;
                }
            }
            MgrAction::McpFormat => {
                let text = self.paste_text(cx);
                let mut tidy = None;
                if let Some(w) = &mut self.mcp_wizard {
                    w.paste = text;
                    match w.apply_paste() {
                        Ok(()) => {
                            w.error = None;
                            w.sync_paste_from_form();
                            tidy = Some(w.paste.clone());
                        }
                        Err(e) => w.error = Some(e),
                    }
                }
                if let Some(tidy) = tidy {
                    self.seed_paste(tidy, cx);
                    if let Some(w) = self.mcp_wizard.take() {
                        self.seed_mcp_inputs(&w, cx); // parsed fields -> form
                        self.mcp_wizard = Some(w);
                    }
                }
            }
            MgrAction::McpSubmit => {
                self.read_mcp_inputs(cx);
                let paste_mode = self
                    .mcp_wizard
                    .as_ref()
                    .is_some_and(|w| w.mode == EditMode::Paste);
                if paste_mode {
                    let text = self.paste_text(cx);
                    if let Some(w) = &mut self.mcp_wizard {
                        w.paste = text;
                        if let Err(e) = w.apply_paste() {
                            w.error = Some(e);
                            return;
                        }
                    }
                }
                let Some(w) = &mut self.mcp_wizard else {
                    return;
                };
                if let Err(e) = mcp_wizard::validate_fields(w) {
                    w.error = Some(e);
                    return;
                }
                let (name, wire, config) = (w.name(), w.transport_wire(), w.config());
                self.with_ready_backend(|b| b.add_mcp_server(&name, wire, &config));
                self.toast(format!("Added MCP server {name}"), accent);
                self.mcp_wizard = None;
                self.mgr_last_request = None; // refresh promptly (egui semantics)
                self.mgr_upkeep();
            }
            _ => {}
        }
    }

    pub(crate) fn seed_mcp_inputs(&self, w: &mcp_wizard::Wizard, cx: &mut gpui::Context<Self>) {
        self.seed(F_NAME, w.name.clone(), cx);
        self.seed(F_B, w.command.clone(), cx);
        self.seed(F_C, w.args.clone(), cx);
        self.seed(F_D, pairs_to_lines(&w.env), cx);
        self.seed(F_E, w.url.clone(), cx);
        self.seed(F_F, pairs_to_lines(&w.headers), cx);
    }

    fn read_mcp_inputs(&mut self, cx: &mut gpui::Context<Self>) {
        let name = self.mgr_input_text(F_NAME, cx);
        let command = self.mgr_input_text(F_B, cx);
        let args = self.mgr_input_text(F_C, cx);
        let env = lines_to_pairs(&self.mgr_input_text(F_D, cx));
        let url = self.mgr_input_text(F_E, cx);
        let headers = lines_to_pairs(&self.mgr_input_text(F_F, cx));
        if let Some(w) = &mut self.mcp_wizard {
            w.name = name;
            w.command = command;
            w.args = args;
            w.env = env;
            w.url = url;
            w.headers = headers;
        }
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Status-dot color for a server's lifecycle state (egui `state_color`).
fn state_color(t: &Tokens, state: &str) -> Rgba {
    match state {
        "running" => t.run,
        "starting" | "stopping" => t.paused,
        "error" => t.error,
        "quarantined" => t.think,
        _ => t.dim, // stopped / unknown
    }
}

pub(crate) fn body(args: &MgrArgs, ws: &Workspace) -> AnyElement {
    let t = args.t;
    if let Some(w) = args.mcp_wizard {
        return wizard_body(args, w);
    }
    match &ws.mcp_servers {
        None => center_hint(&t, &["Loading MCP servers..."]),
        Some(servers) if servers.is_empty() => center_hint(
            &t,
            &[
                "No MCP servers registered yet.",
                "Use \"Add MCP server\" to connect your first one.",
            ],
        ),
        Some(servers) => div()
            .id("mcp-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(
                servers
                    .iter()
                    .enumerate()
                    .map(|(i, s)| server_row(args, i, s)),
            )
            .into_any_element(),
    }
}

/// One server row: status dot, name, transport, error flag, summary, switch.
fn server_row(args: &MgrArgs, i: usize, s: &crate::backend::McpServerInfo) -> AnyElement {
    let t = args.t;
    let color = state_color(&t, &s.state);
    let mut hover = format!("{} - {}", s.name, s.state);
    if !s.error.is_empty() {
        hover.push_str(&format!("\n{}", s.error));
    }
    // Optimistic value: the pending toggle wins until fresh data.
    let on = args.pending.get(&s.name).copied().unwrap_or(s.enabled);
    let name = s.name.clone();
    div()
        .id(("mcp-row", i as u64))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.card)
        .border_1()
        .border_color(t.line_soft)
        .child(
            div()
                .id(("mcp-dot", i as u64))
                .size(px(8.))
                .flex_none()
                .rounded_full()
                .bg(color)
                .tooltip(widgets::text_tip(hover)),
        )
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .text_color(t.text)
                .child(s.name.clone()),
        )
        .child(small(&t, s.transport.clone(), t.weak))
        .children((!s.error.is_empty()).then(|| {
            div()
                .id(("mcp-err", i as u64))
                .text_size(px(12.))
                .text_color(t.error)
                .child("!")
                .tooltip(widgets::text_tip(s.error.clone()))
        }))
        .child(div().flex_1())
        .children((!s.summary.is_empty()).then(|| {
            div()
                .id(("mcp-summary", i as u64))
                .max_w(px(280.))
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(t.weak)
                .child(s.summary.clone())
                .tooltip(widgets::text_tip(s.summary.clone()))
        }))
        .child(
            div()
                .id(("mcp-switch", i as u64))
                .cursor_pointer()
                .child(switch(&t, on))
                .tooltip(widgets::text_tip(
                    if on {
                        "Stop this server"
                    } else {
                        "Start this server"
                    }
                    .into(),
                ))
                .on_click(act(&args.root, MgrAction::McpToggle(name, !on))),
        )
        .into_any_element()
}

fn wizard_body(args: &MgrArgs, w: &mcp_wizard::Wizard) -> AnyElement {
    let t = args.t;
    let paste = w.mode == EditMode::Paste;

    let body: AnyElement = if paste {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(small(
                &t,
                "Paste a server entry, e.g. {\"my-server\": {\"command\": \"npx\", ...}}:",
                t.weak,
            ))
            .child(paste_panel(args))
            .child(small(
                &t,
                "An outer mcpServers wrapper is unwrapped; the transport is read from a \
                 \"type\" field or inferred from command/url. Format tidies; Add validates.",
                t.dim,
            ))
            .into_any_element()
    } else {
        match w.step {
            0 => div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(small(&t, "How does this MCP server run?", t.weak))
                .children(
                    [
                        (0u8, Transport::Stdio),
                        (1, Transport::Sse),
                        (2, Transport::Http),
                    ]
                    .into_iter()
                    .map(|(ix, tr)| {
                        option_card(&t, w.transport == tr, tr.label(), tr.blurb())
                            .id(("mcp-transport", ix as u64))
                            .on_click(act(&args.root, MgrAction::McpTransport(ix)))
                            .into_any_element()
                    }),
                )
                .into_any_element(),
            1 => {
                let stdio = w.transport == Transport::Stdio;
                let mut col = div().flex().flex_col().gap_1p5().child(field(
                    &t,
                    "name",
                    args.inputs.get(F_NAME),
                    false,
                ));
                if stdio {
                    col = col
                        .child(field(&t, "command", args.inputs.get(F_B), false))
                        .child(field(&t, "args (one per line)", args.inputs.get(F_C), true))
                        .child(field(
                            &t,
                            "env (KEY=VALUE per line)",
                            args.inputs.get(F_D),
                            true,
                        ));
                } else {
                    col = col
                        .child(field(&t, "url", args.inputs.get(F_E), false))
                        .child(field(
                            &t,
                            "headers (KEY=VALUE per line)",
                            args.inputs.get(F_F),
                            true,
                        ));
                }
                col.into_any_element()
            }
            _ => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(small(&t, "Review and confirm:", t.weak))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child(w.name()),
                        )
                        .child(small(&t, w.transport_wire(), t.weak)),
                )
                .child(mono_block(
                    &t,
                    serde_json::to_string_pretty(&w.config()).unwrap_or_default(),
                ))
                .child(small(
                    &t,
                    "The server is registered globally and enabled; toggle it off any time.",
                    t.dim,
                ))
                .into_any_element(),
        }
    };

    let footer = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            widgets::btn(&t, "Cancel")
                .id("mcp-cancel")
                .on_click(act(&args.root, MgrAction::McpWizardCancel)),
        )
        .child(div().flex_1())
        .children(paste.then(|| {
            widgets::btn(&t, "Format")
                .id("mcp-format")
                .on_click(act(&args.root, MgrAction::McpFormat))
        }))
        .children((!paste && w.step > 0).then(|| {
            widgets::btn(&t, "\u{2190} Back")
                .id("mcp-back")
                .on_click(act(&args.root, MgrAction::McpStep(-1)))
        }))
        .children((!paste && w.step < 2).then(|| {
            widgets::btn(&t, "Next \u{2192}")
                .id("mcp-next")
                .on_click(act(&args.root, MgrAction::McpStep(1)))
        }))
        .children((paste || w.step == 2).then(|| {
            widgets::primary_btn(&t, "\u{2713} Add server")
                .id("mcp-submit")
                .on_click(act(&args.root, MgrAction::McpSubmit))
        }));

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(small(&t, "Add MCP server", t.text))
                .children(
                    (!paste).then(|| step_strip(&t, &["Transport", "Details", "Review"], w.step)),
                )
                .child(div().flex_1())
                .child(mode_toggle(
                    args,
                    paste,
                    MgrAction::McpMode(false),
                    MgrAction::McpMode(true),
                )),
        )
        .children(w.error.clone().map(|e| small(&t, e, t.error)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .id("mcp-wiz-scroll")
                .overflow_y_scroll()
                .child(body),
        )
        .child(footer)
        .into_any_element()
}
