//! The chat composer: input box, inline completion, commands menu, pending
//! interactive prompts, and the agent/model/puppy-name controls.

use eframe::egui;

use crate::backend::CommandInfo;

use super::Workspace;

impl Workspace {
    pub(crate) fn render_agent_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen: Option<String> = None;
        let current = if self.agent.is_empty() {
            "agent"
        } else {
            &self.agent
        };
        egui::ComboBox::from_id_salt(("agent-combo", self.id.0))
            .selected_text(format!("🐶 {current}"))
            .show_ui(ui, |ui| {
                for a in &self.agents {
                    let label = if a.display_name.is_empty() {
                        a.name.clone()
                    } else {
                        a.display_name.clone()
                    };
                    let resp = ui
                        .selectable_label(a.current, label)
                        .on_hover_text(&a.description);
                    if resp.clicked() && !a.current {
                        chosen = Some(a.name.clone());
                    }
                }
            });
        if let Some(name) = chosen
            && let Some(backend) = &self.backend
        {
            backend.set_agent(&name);
        }
    }

    pub(crate) fn render_model_picker(&mut self, ui: &mut egui::Ui) {
        let mut chosen: Option<String> = None;
        let current = if self.model.is_empty() {
            "model"
        } else {
            &self.model
        };
        egui::ComboBox::from_id_salt(("model-combo", self.id.0))
            .selected_text(current.to_string())
            .show_ui(ui, |ui| {
                for m in &self.models {
                    let resp = ui
                        .selectable_label(m.current, &m.name)
                        .on_hover_text(&m.description);
                    if resp.clicked() && !m.current {
                        chosen = Some(m.name.clone());
                    }
                }
            });
        if let Some(name) = chosen
            && let Some(backend) = &self.backend
        {
            backend.set_model(&name);
        }
    }

    pub(crate) fn render_composer(&mut self, ui: &mut egui::Ui) {
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
                    self.restart(ui.ctx());
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

        let mut stop = false;
        let mut do_submit = false;
        let mut do_steer = false;
        let mut do_pause = false;
        let mut paste_image = false;
        let mut open_files = false;
        ui.horizontal(|ui| {
            // Commands menu to the left of the input box.
            self.render_commands_menu(ui);
            if ui
                .button("Image")
                .on_hover_text("Attach an image from the clipboard (or press Ctrl+V)")
                .clicked()
            {
                paste_image = true;
            }
            if ui
                .button("File")
                .on_hover_text("Browse workspace files and @-reference one in your message")
                .clicked()
            {
                open_files = true;
            }

            let running = self.running;
            // While a turn runs, the input box steers; otherwise it sends.
            let input_enabled = self.ready;
            let hint = if !self.ready {
                "Waiting for Code Puppy to start…".to_string()
            } else if running {
                format!("Steer {} mid-turn… (Enter to steer)", self.puppy_name)
            } else {
                format!(
                    "Message {}…  (/ for commands, @ for files)",
                    self.puppy_name
                )
            };
            // Reserve room on the right for the action buttons.
            let reserve = if running { 250.0 } else { 70.0 };
            let field = ui.add_enabled(
                input_enabled,
                egui::TextEdit::singleline(&mut self.input)
                    // Stable, absolute id so focus survives the completion popup
                    // appearing/disappearing above us (which shifts auto-ids).
                    .id(egui::Id::new(("composer-input", self.id.0)))
                    .desired_width((ui.available_width() - reserve).max(60.0))
                    .hint_text(hint),
            );
            if self.request_input_focus {
                field.request_focus();
                self.request_input_focus = false;
            }
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if running {
                // Steer delivery mode toggle (now = interrupt, queue = after turn).
                let mode_label = if self.steer_queue_mode {
                    "📨 queue"
                } else {
                    "🎯 now"
                };
                if ui
                    .selectable_label(false, mode_label)
                    .on_hover_text(
                        "Steer delivery — now: interrupt mid-turn · queue: after this turn",
                    )
                    .clicked()
                {
                    self.steer_queue_mode = !self.steer_queue_mode;
                }
                let steer = ui
                    .add_enabled(input_enabled, egui::Button::new("Steer"))
                    .on_hover_text("Send this as a steering message")
                    .clicked();
                let pause_label = if self.paused {
                    "▶ Resume"
                } else {
                    "⏸ Pause"
                };
                if ui
                    .button(pause_label)
                    .on_hover_text("Pause/resume the turn at the next safe point")
                    .clicked()
                {
                    do_pause = true;
                }
                if ui
                    .button("⏹ Stop")
                    .on_hover_text("Cancel the running turn")
                    .clicked()
                {
                    stop = true;
                }
                if !apply && (enter || steer) {
                    do_steer = true;
                }
            } else {
                let send = ui
                    .add_enabled(input_enabled, egui::Button::new("Send"))
                    .clicked();
                if !apply && (enter || send) {
                    do_submit = true;
                }
            }
        });

        if do_submit {
            self.submit();
            self.request_input_focus = true;
        }
        if do_steer {
            self.steer();
            self.request_input_focus = true;
        }
        if do_pause {
            self.toggle_pause();
        }
        if stop {
            if let Some(backend) = &self.backend {
                backend.cancel();
            }
            self.status_line = "Cancelling…".to_string();
        }
        if apply {
            self.apply_completion();
            self.request_input_focus = true;
        }
        if paste_image {
            self.try_paste_image(ui.ctx());
        }
        if open_files {
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

    pub(crate) fn render_completion_popup(&mut self, ui: &mut egui::Ui) {
        let mut clicked: Option<usize> = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
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
                                            ui.label(egui::RichText::new(&c.meta).weak().small());
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
