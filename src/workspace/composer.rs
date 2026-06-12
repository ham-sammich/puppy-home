//! The shared composer core: input state, inline completion (the slash
//! palette), commands menu, attachments, and the Classic style's layout.
//! Every composer style (see `dock.rs`) is a skin over this one state and
//! send path: `composer_prelude` → style layout → `composer_epilogue`.

use eframe::egui;

use crate::backend::CommandInfo;

use super::Workspace;

/// Side effects a composer style requests this frame. The dock routes them
/// through [`Workspace::composer_epilogue`] so every skin shares one send path.
#[derive(Default)]
pub(crate) struct ComposerFx {
    /// Send the input (steers while a turn runs).
    pub(crate) submit: bool,
    /// Attach an image from the clipboard.
    pub(crate) paste: bool,
    /// Open the @file browser.
    pub(crate) files: bool,
}

impl Workspace {
    /// Stable id for the composer input, shared by every style so focus,
    /// paste detection, and history recall survive style switches.
    pub(crate) fn composer_input_id(&self) -> egui::Id {
        egui::Id::new(("composer-input", self.id.0))
    }

    /// Placeholder text for the input field (any style).
    pub(crate) fn composer_hint(&self) -> String {
        if !self.ready {
            "Waiting for Code Puppy to start\u{2026}".to_string()
        } else if self.running {
            format!("Steer {} mid-turn\u{2026}", self.puppy_name)
        } else {
            format!(
                "Message {}\u{2026}  (/ for commands, @ for files)",
                self.puppy_name
            )
        }
    }

    /// Consume a bare Enter while the input has focus. Multiline styles call
    /// this BEFORE adding their field so Enter sends and Shift+Enter newlines.
    pub(crate) fn take_enter(&self, ui: &mut egui::Ui) -> bool {
        ui.memory(|m| m.has_focus(self.composer_input_id()))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
    }

    /// One-shot focus request, honored by whichever style draws the field.
    pub(crate) fn focus_if_requested(&mut self, field: &egui::Response) {
        if self.request_input_focus {
            field.request_focus();
            self.request_input_focus = false;
        }
    }

    /// Shared pre-input machinery for every composer style: restart banner,
    /// image paste, history recall, completion keys, the slash palette, and
    /// attachment thumbnails. Returns whether a completion should be applied
    /// instead of submitting.
    pub(crate) fn composer_prelude(&mut self, ui: &mut egui::Ui) -> bool {
        // A crashed/exited sidecar leaves no backend; offer a one-click restart
        // that relaunches it and re-attaches this workspace's conversation.
        if self.backend.is_none() {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(240, 130, 130),
                    "Code Puppy stopped.",
                );
                if ui
                    .button("Restart Code Puppy")
                    .on_hover_text("Relaunch the sidecar and restore this conversation")
                    .clicked()
                {
                    self.restart(crate::waker::egui_waker(ui.ctx()));
                }
            });
            ui.add_space(4.0);
        }

        // Image paste: egui-winit treats Ctrl/Cmd+V as a paste *command* and
        // early-returns, so the V key *press* is never delivered to egui at all
        // (plain text paste arrives as a separate Paste event). The key
        // *release* IS delivered, so we trigger on release while the composer
        // input holds focus.
        let input_id = egui::Id::new(("composer-input", self.id.0));
        if ui.memory(|m| m.has_focus(input_id))
            && ui.input(|i| i.modifiers.command && i.key_released(egui::Key::V))
        {
            self.try_paste_image(ui.ctx());
        }

        // Shell-style history: Up/Down in the (focused) input recalls previously
        // sent messages for editing/resending. The completion popup gets the
        // arrows when it's open; a singleline edit has no other use for them.
        if ui.memory(|m| m.has_focus(input_id))
            && !self.comp_visible
            && !self.input_history.is_empty()
        {
            let m = egui::Modifiers::NONE;
            let mut recalled = false;
            if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowUp)) {
                match self.history_pos {
                    None => {
                        self.history_stash = self.input.clone();
                        self.history_pos = Some(self.input_history.len() - 1);
                    }
                    Some(0) => {}
                    Some(p) => self.history_pos = Some(p - 1),
                }
                if let Some(p) = self.history_pos {
                    self.input = self.input_history[p].clone();
                    recalled = true;
                }
            } else if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowDown))
                && let Some(p) = self.history_pos
            {
                if p + 1 < self.input_history.len() {
                    self.history_pos = Some(p + 1);
                    self.input = self.input_history[p + 1].clone();
                } else {
                    // Walked past the newest entry: restore the stashed draft.
                    self.history_pos = None;
                    self.input = std::mem::take(&mut self.history_stash);
                }
                recalled = true;
            }
            if recalled {
                // Park the caret at the end of the recalled line and keep the
                // completion engine from treating this as fresh typing.
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), input_id) {
                    let end = egui::text::CCursor::new(self.input.chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
                    state.store(ui.ctx(), input_id);
                }
                self.last_query = self.input.clone();
                self.comp_visible = false;
            }
        }

        let mut apply = false;
        if self.comp_visible && !self.completions.is_empty() {
            let len = self.completions.len();
            let m = egui::Modifiers::NONE;
            if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowDown)) {
                self.comp_selected = (self.comp_selected + 1) % len;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::ArrowUp)) {
                self.comp_selected = (self.comp_selected + len - 1) % len;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Escape)) {
                self.comp_visible = false;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Tab)) {
                apply = true;
            }
            if ui.input_mut(|i| i.consume_key(m, egui::Key::Enter)) {
                apply = true;
            }
        }

        if self.comp_visible {
            self.render_completion_popup(ui);
        }

        self.render_attachments(ui);
        apply
    }

    /// Classic style: ⌘Commands · Image · Files · input · Send, plus a second
    /// row of Terminal / Sessions toggles and the agent/model switchers.
    pub(crate) fn render_style_classic(&mut self, ui: &mut egui::Ui, apply: bool) -> ComposerFx {
        let mut fx = ComposerFx::default();
        ui.horizontal(|ui| {
            self.render_commands_menu(ui);
            if ui
                .button("\u{1f5bc} Image")
                .on_hover_text("Attach an image from the clipboard (or press Ctrl+V)")
                .clicked()
            {
                fx.paste = true;
            }
            if ui
                .button("\u{1f4ce} Files")
                .on_hover_text("Browse workspace files and @-reference one in your message")
                .clicked()
            {
                fx.files = true;
            }
            let hint = self.composer_hint();
            let input_id = self.composer_input_id();
            let field = ui.add_enabled(
                self.ready,
                egui::TextEdit::singleline(&mut self.input)
                    // Stable, absolute id so focus survives the completion popup
                    // appearing/disappearing above us (which shifts auto-ids).
                    .id(input_id)
                    .desired_width((ui.available_width() - 70.0).max(60.0))
                    .hint_text(hint),
            );
            self.focus_if_requested(&field);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let label = if self.running { "Steer" } else { "Send" };
            let send = ui
                .add_enabled(self.ready, egui::Button::new(label))
                .clicked();
            if !apply && (enter || send) {
                fx.submit = true;
            }
        });
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            self.terminal_toggle(ui);
            self.sessions_toggle(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.model_switcher(ui);
                self.agent_switcher(ui);
            });
        });
        fx
    }

    /// Shared post-input dispatch: apply a completion, or send (steer while a
    /// turn runs), plus the paste / file-browser side actions.
    pub(crate) fn composer_epilogue(&mut self, ui: &mut egui::Ui, apply: bool, fx: ComposerFx) {
        if apply {
            self.apply_completion();
            self.request_input_focus = true;
        } else if fx.submit {
            if self.running {
                self.steer();
            } else {
                self.submit();
            }
            self.request_input_focus = true;
        }
        if fx.paste {
            self.try_paste_image(ui.ctx());
        }
        if fx.files {
            self.open_file_browser();
        }
        self.maybe_request_completion();
    }

    /// The puppy's name + inline rename (writes Code Puppy's global config).
    pub(crate) fn render_puppy_name(&mut self, ui: &mut egui::Ui) {
        let mut commit: Option<String> = None;
        let mut cancel = false;
        let mut begin = false;
        if let Some(edit) = self.name_edit.as_mut() {
            let resp = ui.add(
                egui::TextEdit::singleline(edit)
                    .desired_width(110.0)
                    .hint_text("puppy name"),
            );
            resp.request_focus();
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.small_button("✓").on_hover_text("Rename").clicked() || enter {
                commit = Some(edit.trim().to_string());
            }
            if ui.small_button("✕").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        } else if ui
            .button(format!("🐶 {}", self.puppy_name))
            .on_hover_text(format!(
                "Your puppy is named {} (you are {}). Click to rename.",
                self.puppy_name, self.owner_name
            ))
            .clicked()
        {
            begin = true;
        }

        if begin {
            self.name_edit = Some(self.puppy_name.clone());
        }
        if cancel {
            self.name_edit = None;
        }
        if let Some(name) = commit {
            self.name_edit = None;
            if !name.is_empty() && name != self.puppy_name {
                self.puppy_name = name.clone();
                if let Some(backend) = &self.backend {
                    backend.set_puppy_name(&name);
                }
            }
        }
    }

    /// Thumbnails of images attached to the next prompt, each removable.
    fn render_attachments(&mut self, ui: &mut egui::Ui) {
        if self.pending_images.is_empty() {
            return;
        }
        let mut remove: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Attached:").weak().small());
            for (i, img) in self.pending_images.iter().enumerate() {
                let [w, h] = img.size;
                let scale = 48.0 / (w.max(h) as f32).max(1.0);
                let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                ui.add(egui::Image::new(&img.texture).fit_to_exact_size(size))
                    .on_hover_text(format!("{w}x{h} - attached to your next message"));
                if ui.small_button("x").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            }
        });
        if let Some(i) = remove {
            self.pending_images.remove(i);
        }
        ui.add_space(2.0);
    }

    /// Grab an image from the clipboard (if any) and attach it to the next prompt.
    fn try_paste_image(&mut self, ctx: &egui::Context) {
        let Some(img) = super::clipboard::read_clipboard_image() else {
            return;
        };
        let Some(png_base64) =
            super::clipboard::encode_png_base64(img.width, img.height, &img.rgba)
        else {
            return;
        };
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.rgba);
        let texture = ctx.load_texture(
            format!("paste-{}-{}", self.id.0, self.pending_images.len()),
            color,
            egui::TextureOptions::LINEAR,
        );
        let (w, h) = (img.width, img.height);
        self.pending_images.push(super::PendingImage {
            png_base64,
            size: [w, h],
            texture,
        });
        self.status_line = format!("Attached image ({w}x{h}).");
    }

    pub(crate) fn apply_completion(&mut self) {
        let Some(item) = self.completions.get(self.comp_selected).cloned() else {
            return;
        };
        let remove = (-item.start_position).max(0) as usize;
        let char_len = self.input.chars().count();
        let keep = char_len.saturating_sub(remove);
        let prefix: String = self.input.chars().take(keep).collect();
        self.input = format!("{prefix}{}", item.text);
        self.comp_visible = false;
        self.last_query = self.input.clone();
    }

    /// The slash palette: sidecar-provided completions (commands, @files)
    /// styled per the mock — mono command + weak hint rows in a popup frame.
    pub(crate) fn render_completion_popup(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<usize> = None;
        egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(6.0)
            .shadow(egui::epaint::Shadow {
                offset: [0, 10],
                blur: 28,
                spread: 0,
                color: egui::Color32::from_black_alpha(150),
            })
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Commands \u{00b7} \u{2191}\u{2193} then Tab or Enter \u{00b7} Esc closes",
                    )
                    .weak()
                    .small(),
                );
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, c) in self.completions.iter().enumerate() {
                            let selected = i == self.comp_selected;
                            let resp = ui
                                .horizontal(|ui| {
                                    let lab = ui.selectable_label(
                                        selected,
                                        egui::RichText::new(&c.display).monospace(),
                                    );
                                    if !c.meta.is_empty() {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(&c.meta).weak().small(),
                                                );
                                            },
                                        );
                                    }
                                    lab
                                })
                                .inner;
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            if selected {
                                resp.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });
            });
        if let Some(i) = clicked {
            self.comp_selected = i;
            self.apply_completion();
            self.request_input_focus = true;
        }
    }

    pub(crate) fn maybe_request_completion(&mut self) {
        if !self.ready || self.running {
            self.comp_visible = false;
            return;
        }
        if self.input == self.last_query {
            return;
        }
        self.last_query = self.input.clone();
        let completable = self.input.starts_with('/') || self.input.contains('@');
        if !completable {
            self.comp_visible = false;
            self.completions.clear();
            return;
        }
        if let Some(backend) = &self.backend {
            self.comp_request_id =
                backend.request_completion(&self.input, self.input.chars().count());
        }
    }

    /// Commands menu with "smart" behavior: arg-less, non-interactive commands
    /// run on click; commands that take arguments (or open a terminal-only
    /// picker) are dropped into the composer for you to complete.
    pub(crate) fn render_commands_menu(&mut self, ui: &mut egui::Ui) {
        enum Pick {
            Run(String),
            Insert(String),
        }
        let mut pick: Option<Pick> = None;
        let enabled = self.ready && !self.running && !self.commands.is_empty();
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Commands ▾", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        let mut last_cat = "";
                        for c in &self.commands {
                            if c.category != last_cat {
                                if !last_cat.is_empty() {
                                    ui.separator();
                                }
                                ui.label(
                                    egui::RichText::new(c.category.to_uppercase())
                                        .small()
                                        .weak(),
                                );
                                last_cat = &c.category;
                            }
                            let hover = format!("{}\n{}", c.usage, c.description);
                            if ui
                                .button(format!("/{}", c.name))
                                .on_hover_text(hover)
                                .clicked()
                            {
                                pick = Some(if command_needs_input(c) {
                                    Pick::Insert(c.name.clone())
                                } else {
                                    Pick::Run(c.name.clone())
                                });
                                ui.close();
                            }
                        }
                    });
            });
        });
        match pick {
            Some(Pick::Insert(name)) => {
                self.input = format!("/{name} ");
                self.request_input_focus = true;
            }
            Some(Pick::Run(name)) => {
                self.dispatch_command(&format!("/{name}"));
            }
            None => {}
        }
    }
}

/// A command "needs input" if its usage shows a placeholder (`<...>` / `[...]`)
/// or it's a known interactive picker that can't run headless.
pub(crate) fn command_needs_input(c: &CommandInfo) -> bool {
    const INTERACTIVE: &[&str] = &[
        "agent",
        "model",
        "mcp",
        "add_model",
        "diff",
        "colors",
        "model_settings",
        "set",
        "tutorial",
        "judges",
    ];
    c.usage.contains('<') || c.usage.contains('[') || INTERACTIVE.contains(&c.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::command_needs_input;
    use crate::backend::CommandInfo;

    fn cmd(name: &str, usage: &str) -> CommandInfo {
        CommandInfo {
            name: name.into(),
            usage: usage.into(),
            description: String::new(),
            category: String::new(),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn placeholder_usage_needs_input() {
        assert!(command_needs_input(&cmd("foo", "/foo <arg>")));
        assert!(command_needs_input(&cmd("bar", "/bar [opt]")));
    }

    #[test]
    fn interactive_commands_need_input() {
        assert!(command_needs_input(&cmd("agent", "")));
        assert!(command_needs_input(&cmd("model", "")));
    }

    #[test]
    fn plain_command_runs_directly() {
        assert!(!command_needs_input(&cmd("help", "")));
        assert!(!command_needs_input(&cmd("clear", "/clear")));
    }
}
