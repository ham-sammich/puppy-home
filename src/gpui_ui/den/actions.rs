//! Den interactions + the relay glue that drives [`DenConn`]: join/leave,
//! feed, kanban ops, plan sharing, the roster/presence broadcasts (rate-
//! limited, change-gated — ported from the egui app's `pack_sync`), and the
//! per-drain event pump.

use std::time::{Duration, Instant};

use gpui::Context;
use puppy_relay::protocol::{Presence, RoomAgentInfo, TaskColumn};

use crate::pack::{DenState, PackClient, PackEvent};
use crate::workspace::{InstanceStatus, SPARK_SAMPLES, SparkRing};

use super::super::{DashAction, RootView, Screen};
use super::{DenConn, DenPop, DenSeg, TaskTarget};

/// Minimum spacing between den roster broadcasts (egui pack_sync parity).
const DEN_ROSTER_EVERY: Duration = Duration::from_millis(2500);
/// Idle threshold for the presence heuristic.
const PRESENCE_IDLE_AFTER: Duration = Duration::from_secs(300);

#[derive(Clone, Debug)]
pub enum DenAction {
    /// Open the Den screen (join form when not connected).
    Show,
    JoinSubmit,
    /// Spawn a local puppy-relay and auto-join it (QW6).
    HostSubmit,
    /// Kill the locally hosted relay (ends the den for everyone).
    StopHost,
    Leave,
    SetSeg(DenSeg),
    CopyRoom,
    Invite,
    FeedSend,
    FeedShowOlder,
    Nudge(String),
    /// Open one of YOUR agents from the roster (dir name lookup).
    OpenOwn(String),
    TogglePop(DenPop),
    ClosePop,
    TaskAdd(TaskColumn),
    TaskMove(u64, TaskColumn),
    TaskAssignMe(u64),
    TaskUnassign(u64),
    TaskRetitle(u64),
    TaskDelete(u64),
    /// Submit the task input (create or retitle, per `den_task_target`).
    TaskInputSubmit,
    PlanShare(std::path::PathBuf),
    PlanUnshare,
}

impl RootView {
    pub(crate) fn dispatch_den(&mut self, action: DenAction, cx: &mut Context<Self>) {
        let accent = self.tokens.accent;
        match action {
            DenAction::Show => {
                self.ensure_join_inputs(cx);
                self.screen = Screen::Den;
            }
            DenAction::JoinSubmit => self.den_join(cx),
            DenAction::HostSubmit => self.den_host_start(cx),
            DenAction::StopHost => {
                if let Some(host) = self.den_host.take() {
                    host.stop();
                }
                if let Some(den) = self.den.take() {
                    den.client.leave();
                }
                self.toast(
                    format!(
                        "Stopped hosting \u{2014} the {} is closed",
                        crate::pack::DEN_LABEL
                    ),
                    accent,
                );
            }
            DenAction::Leave => {
                // Leaving while hosting also stops the relay (the room
                // lives in this process; no zombie hosts).
                if let Some(host) = self.den_host.take() {
                    host.stop();
                }
                if let Some(den) = self.den.take() {
                    den.client.leave();
                    self.toast(
                        format!("Left the {} \u{b7} {}", crate::pack::DEN_LABEL, den.room),
                        accent,
                    );
                }
                self.screen = Screen::Dashboard;
                self.den_pop = None;
            }
            DenAction::SetSeg(seg) => self.den_seg = seg,
            DenAction::CopyRoom => {
                if let Some(den) = &self.den {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(den.room.clone()));
                    self.toast(format!("Room code {} copied", den.room), accent);
                }
            }
            DenAction::Invite => {
                if let Some(den) = &self.den {
                    let line = format!(
                        "Join my {}: room {} on relay {} (puppy-home \u{2192} Join Den)",
                        crate::pack::DEN_LABEL,
                        den.room,
                        den.addr
                    );
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(line));
                    self.toast("Invite line copied".to_string(), accent);
                }
            }
            DenAction::FeedSend => {
                let Some(input) = self.den_feed_input.clone() else {
                    return;
                };
                let text = input.read(cx).text().trim().to_string();
                if text.is_empty() {
                    return;
                }
                if let Some(den) = &self.den {
                    den.client.chat(&text);
                }
                input.update(cx, |i, cx| i.clear(cx));
            }
            DenAction::FeedShowOlder => self.den_show_all_feed = true,
            DenAction::Nudge(puppy) => {
                if let Some(den) = &self.den {
                    let me = self.puppy_name();
                    den.client.puppy_msg(
                        &me,
                        &puppy,
                        false,
                        &format!("\u{1f44b} nudge from {}", den.user),
                    );
                    self.toast(format!("Nudged {puppy}"), accent);
                }
            }
            DenAction::OpenOwn(dir) => {
                let found = self.supervisor.iter().find(|w| w.name == dir).map(|w| w.id);
                if let Some(id) = found {
                    self.dispatch(DashAction::Open(id), cx);
                }
            }
            DenAction::TogglePop(pop) => {
                self.den_pop = if self.den_pop.as_ref() == Some(&pop) {
                    None
                } else {
                    Some(pop)
                };
            }
            DenAction::ClosePop => self.den_pop = None,
            DenAction::TaskAdd(col) => {
                self.ensure_task_input(cx);
                self.den_task_target = match self.den_task_target {
                    Some(TaskTarget::Add(c)) if c == col => None,
                    _ => Some(TaskTarget::Add(col)),
                };
                self.clear_task_input(cx);
            }
            DenAction::TaskMove(id, col) => {
                self.den_pop = None;
                if let Some(den) = &self.den {
                    den.client.task_move(id, col);
                }
            }
            DenAction::TaskAssignMe(id) => {
                self.den_pop = None;
                if let Some(den) = &self.den {
                    let user = den.user.clone();
                    den.client.task_assign(id, &user);
                }
            }
            DenAction::TaskUnassign(id) => {
                self.den_pop = None;
                if let Some(den) = &self.den {
                    den.client.task_assign(id, "");
                }
            }
            DenAction::TaskRetitle(id) => {
                self.den_pop = None;
                self.ensure_task_input(cx);
                self.den_task_target = Some(TaskTarget::Retitle(id));
                self.clear_task_input(cx);
            }
            DenAction::TaskDelete(id) => {
                self.den_pop = None;
                if let Some(den) = &self.den {
                    den.client.task_delete(id);
                    self.toast("Card deleted".to_string(), accent);
                }
            }
            DenAction::TaskInputSubmit => {
                let Some(input) = self.den_task_input.clone() else {
                    return;
                };
                let text = input.read(cx).text().trim().to_string();
                if text.is_empty() {
                    self.den_task_target = None;
                    return;
                }
                match (self.den_task_target.take(), &self.den) {
                    (Some(TaskTarget::Add(col)), Some(den)) => {
                        den.client.task_create(&text, col, "", false);
                    }
                    (Some(TaskTarget::Retitle(id)), Some(den)) => {
                        den.client.task_retitle(id, &text);
                    }
                    _ => {}
                }
                input.update(cx, |i, cx| i.clear(cx));
            }
            DenAction::PlanShare(path) => {
                self.den_pop = None;
                let md = self
                    .supervisor
                    .iter()
                    .find(|w| path.starts_with(&w.root))
                    .map(|w| w.fs_handle())
                    .and_then(|fs| fs.read_to_string(&path).ok());
                match (md, &self.den) {
                    (Some(md), Some(den)) => {
                        let puppy = self.puppy_name();
                        den.client.plan_share(&puppy, &md);
                        self.toast("Plan shared to the den".to_string(), accent);
                    }
                    _ => self.toast("Couldn't read that plans.md".to_string(), accent),
                }
            }
            DenAction::PlanUnshare => {
                if let Some(den) = &self.den {
                    let puppy = self.puppy_name();
                    den.client.plan_unshare(&puppy);
                    self.toast("Plan withdrawn".to_string(), accent);
                }
            }
        }
        cx.notify();
    }

    /// Reset per-den broadcast de-dup state so the FIRST pump after (re)joining
    /// ALWAYS re-broadcasts our roster to the new room. Without this, a second
    /// den in the same app session sees "no change" (stale `den_roster_last`)
    /// and never sends our agents -> everyone (including us) shows
    /// "no agents reported yet".
    fn reset_den_broadcast_state(&mut self) {
        self.den_roster_last.clear();
        self.den_roster_at = None;
    }

    /// Connect to the relay with the join form's values.
    /// Host flow: spawn the relay, then join our own den on localhost.
    fn den_host_start(&mut self, cx: &mut Context<Self>) {
        if self.den_host.is_some() || self.den.is_some() {
            return;
        }
        match super::host::spawn_host() {
            Ok(host) => {
                let room = super::host::room_code();
                let user = std::env::var("USER")
                    .or_else(|_| std::env::var("USERNAME"))
                    .unwrap_or_else(|_| "host".to_string());
                let addr = format!("127.0.0.1:{}", host.port);
                let share = format!("{} \u{b7} room {room}", host.share_addr);
                let puppy = self.puppy_name();
                let user_av = crate::gpui_ui::avatars::inline(
                    self.user_avatar(),
                    crate::gpui_ui::avatars::USER_DEFAULT,
                );
                let puppy_av = crate::gpui_ui::avatars::inline(
                    self.puppy_avatar(),
                    crate::gpui_ui::avatars::PUPPY_DEFAULT,
                );
                match PackClient::connect(
                    &addr,
                    &room,
                    &user,
                    &puppy,
                    &user_av,
                    &puppy_av,
                    self.waker.clone(),
                ) {
                    Ok((client, rx)) => {
                        // Seed the join inputs so the form mirrors reality.
                        self.ensure_join_inputs(cx);
                        for (input, text) in [
                            (&self.den_join_addr, addr.clone()),
                            (&self.den_join_room, room.clone()),
                            (&self.den_join_user, user.clone()),
                        ] {
                            if let Some(i) = input {
                                i.update(cx, |i, cx| i.set_text(&text, cx));
                            }
                        }
                        self.den = Some(DenConn {
                            client,
                            rx,
                            state: DenState::default(),
                            room: room.clone(),
                            addr,
                            user,
                            alive: true,
                            sparks: Default::default(),
                        });
                        self.den_host = Some(host);
                        self.reset_den_broadcast_state();
                        self.den_join_error = None;
                        self.den_show_all_feed = false;
                        self.screen = Screen::Den;
                        self.toast(format!("Hosting \u{b7} share: {share}"), self.tokens.run);
                    }
                    Err(e) => {
                        host.stop();
                        self.den_join_error = Some(format!("relay up but join failed: {e}"));
                    }
                }
            }
            Err(e) => self.den_join_error = Some(e),
        }
    }

    fn den_join(&mut self, cx: &mut Context<Self>) {
        let read = |input: &Option<gpui::Entity<super::super::input::ChatInput>>| {
            input
                .as_ref()
                .map(|i| i.read(cx).text().trim().to_string())
                .unwrap_or_default()
        };
        let addr = read(&self.den_join_addr);
        let room = read(&self.den_join_room);
        let user = read(&self.den_join_user);
        if addr.is_empty() || room.is_empty() || user.is_empty() {
            self.den_join_error = Some("relay address, room and name are all required".into());
            return;
        }
        let puppy = self.puppy_name();
        let user_av = crate::gpui_ui::avatars::inline(
            self.user_avatar(),
            crate::gpui_ui::avatars::USER_DEFAULT,
        );
        let puppy_av = crate::gpui_ui::avatars::inline(
            self.puppy_avatar(),
            crate::gpui_ui::avatars::PUPPY_DEFAULT,
        );
        match PackClient::connect(
            &addr,
            &room,
            &user,
            &puppy,
            &user_av,
            &puppy_av,
            self.waker.clone(),
        ) {
            Ok((client, rx)) => {
                self.den = Some(DenConn {
                    client,
                    rx,
                    state: DenState::default(),
                    room: room.clone(),
                    addr,
                    user,
                    alive: true,
                    sparks: Default::default(),
                });
                self.reset_den_broadcast_state();
                self.den_join_error = None;
                self.den_show_all_feed = false;
                self.screen = Screen::Den;
                self.toast(
                    format!("Joined the {} \u{b7} {room}", crate::pack::DEN_LABEL),
                    self.tokens.run,
                );
            }
            Err(e) => self.den_join_error = Some(e),
        }
    }

    // ------------------------------------------------------------------
    // Drain-loop glue (called once per drain tick)
    // ------------------------------------------------------------------

    /// Fold pending relay events + run the rate-limited broadcasts.
    pub(crate) fn pump_den(&mut self) {
        let Some(den) = &mut self.den else { return };

        // 1. Fold incoming events; derive roster sparklines locally.
        while let Ok(event) = den.rx.try_recv() {
            match event {
                PackEvent::Msg(msg) => {
                    if let puppy_relay::protocol::ServerMsg::Roster { from, agents, .. } = &msg {
                        for a in agents {
                            den.sparks
                                .entry((from.clone(), a.dir.clone()))
                                .or_insert_with(|| SparkRing::new(SPARK_SAMPLES))
                                .push(a.tps);
                        }
                    }
                    den.state.apply(&msg);
                }
                PackEvent::Disconnected => den.alive = false,
            }
        }
        // Drop spark series for members who left (bounded memory).
        let users: std::collections::HashSet<&str> =
            den.state.members.iter().map(|m| m.user.as_str()).collect();
        den.sparks
            .retain(|(user, _), _| users.contains(user.as_str()));

        if !den.alive {
            return;
        }

        // 2. Roster broadcast: rate-limited AND change-gated (pack_sync parity).
        if self
            .den_roster_at
            .map(|at| at.elapsed() >= DEN_ROSTER_EVERY)
            .unwrap_or(true)
        {
            self.den_roster_at = Some(Instant::now());
            let agents: Vec<RoomAgentInfo> = self
                .supervisor
                .iter()
                .map(|w| {
                    let (added, removed) = w.diff_totals();
                    let busy = !matches!(w.status, InstanceStatus::Idle | InstanceStatus::Dead);
                    RoomAgentInfo {
                        puppy: w.puppy_name.clone(),
                        agent: w.agent.clone(),
                        model: w.model.clone(),
                        state: w.status.label().to_string(),
                        verb: w.current_tool.clone().unwrap_or_default(),
                        file: w.last_file().unwrap_or_default().to_string(),
                        dir: w.name.clone(),
                        tps: if busy { w.token_rate as f32 } else { 0.0 },
                        added,
                        removed,
                    }
                })
                .collect();
            let sig = format!("{agents:?}");
            if sig != self.den_roster_last {
                den.client.send_roster(agents);
                self.den_roster_last = sig;
            }
        }

        // 3. Presence: unfocused OR >5min since the last interaction; sent
        //    only when the state flips.
        let idle = !self.window_active || self.last_interaction.elapsed() > PRESENCE_IDLE_AFTER;
        if idle != self.presence_idle {
            self.presence_idle = idle;
            den.client.send_presence(if idle {
                Presence::Idle
            } else {
                Presence::Active
            });
        }
    }
}

// ---------------------------------------------------------------------------
// RootView den glue (aux inputs, probe join, screen body) — same impl,
// housed here for file size.
// ---------------------------------------------------------------------------

use std::path::PathBuf;

use gpui::Entity;
use gpui::prelude::*;

use super::super::input::{ChatInput, InputEvent};
use super::{DenArgs, JoinArgs};

impl RootView {
    /// Create the den's auxiliary inputs on demand (join form + feed).
    pub(crate) fn ensure_join_inputs(&mut self, cx: &mut Context<Self>) {
        if self.den_join_addr.is_some() {
            return;
        }
        let mk = |this: &mut Self, cx: &mut Context<Self>, ph: &str, initial: &str| {
            let entity = cx.new(|cx| ChatInput::new(ph.to_string(), cx));
            if !initial.is_empty() {
                entity.update(cx, |i, cx| i.set_text(initial, cx));
            }
            let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Submitted) {
                    this.dispatch(DashAction::Den(DenAction::JoinSubmit), cx);
                }
            });
            this.chat_subs.push(sub);
            entity
        };
        let addr = std::env::var("PUPPY_RELAY").unwrap_or_else(|_| "127.0.0.1:9220".into());
        let user = std::env::var("USER").unwrap_or_default();
        self.den_join_addr = Some(mk(self, cx, "host:port", &addr));
        self.den_join_room = Some(mk(self, cx, "room code", ""));
        self.den_join_user = Some(mk(self, cx, "your name", &user));

        let feed = cx.new(|cx| ChatInput::new("Message the den\u{2026}", cx));
        let sub = cx.subscribe(&feed, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
                this.dispatch(DashAction::Den(DenAction::FeedSend), cx);
            }
        });
        self.den_feed_input = Some(feed);
        self.chat_subs.push(sub);
    }

    pub(crate) fn ensure_task_input(&mut self, cx: &mut Context<Self>) {
        if self.den_task_input.is_some() {
            return;
        }
        let entity = cx.new(|cx| ChatInput::new("Card title\u{2026}", cx));
        let sub = cx.subscribe(&entity, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Submitted) {
                this.dispatch(DashAction::Den(DenAction::TaskInputSubmit), cx);
            }
        });
        self.den_task_input = Some(entity);
        self.chat_subs.push(sub);
    }

    pub(crate) fn clear_task_input(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = &self.den_task_input {
            input.update(cx, |i, cx| i.clear(cx));
        }
    }

    /// Probe: `PUPPY_GPUI_DEN=addr,room,user` joins a relay room at startup.
    pub(crate) fn maybe_probe_den(&mut self, cx: &mut Context<Self>) {
        if self.den.is_some() {
            return;
        }
        let Ok(spec) = std::env::var("PUPPY_GPUI_DEN") else {
            return;
        };
        // `PUPPY_GPUI_DEN=host` exercises the QW6 hosting path instead.
        if spec == "host" {
            unsafe { std::env::remove_var("PUPPY_GPUI_DEN") };
            self.dispatch(DashAction::Den(DenAction::HostSubmit), cx);
            match (&self.den_host, &self.den_join_error) {
                (Some(h), _) => eprintln!(
                    "[probe] den hosted: share {} \u{b7} joined={}",
                    h.share_addr,
                    self.den.is_some()
                ),
                (None, err) => eprintln!("[probe] den host failed: {err:?}"),
            }
            return;
        }
        let parts: Vec<&str> = spec.splitn(3, ',').collect();
        let [addr, room, user] = parts.as_slice() else {
            return;
        };
        // Fill the join form (so the UI reflects it) and submit.
        self.ensure_join_inputs(cx);
        let set = |e: &Option<Entity<ChatInput>>, v: &str, cx: &mut Context<Self>| {
            if let Some(e) = e {
                let v = v.to_string();
                e.update(cx, |i, cx| i.set_text(v, cx));
            }
        };
        set(&self.den_join_addr.clone(), addr, cx);
        set(&self.den_join_room.clone(), room, cx);
        set(&self.den_join_user.clone(), user, cx);
        // One-shot: clear the env gate so a failed join doesn't loop.
        unsafe { std::env::remove_var("PUPPY_GPUI_DEN") };
        self.dispatch(DashAction::Den(DenAction::JoinSubmit), cx);
        if let Some(err) = &self.den_join_error {
            eprintln!("[probe] den join failed: {err}");
        } else {
            eprintln!("[probe] den joined: room {room} on {addr} as {user}");
        }
    }

    /// The Den screen body: the room when connected, else the join form.
    pub(crate) fn den_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.tokens;
        let entity = cx.entity();
        if let Some(den_conn) = &self.den {
            // Sharable plans: open workspace roots containing a plans.md.
            let sharable: Vec<(String, PathBuf)> = self
                .supervisor
                .iter()
                .filter_map(|w| {
                    let p = w.root.join("plans.md");
                    w.fs_handle().exists(&p).then(|| (w.name.clone(), p))
                })
                .collect();
            return super::den_screen(&DenArgs {
                t,
                root: entity,
                den: den_conn,
                seg: self.den_seg,
                pop: self.den_pop.as_ref(),
                feed_input: self.den_feed_input.as_ref(),
                task_input: self.den_task_input.as_ref(),
                task_target: self.den_task_target,
                show_all_feed: self.den_show_all_feed,
                reduce_motion: self.reduce_motion,
                user_avatar: self.user_avatar().to_string(),
                puppy_avatar: self.puppy_avatar().to_string(),
                sharable_plans: sharable,
                hosting: self.den_host.as_ref().map(|h| h.share_addr.clone()),
            });
        }
        self.ensure_join_inputs(cx);
        super::join_screen(&JoinArgs {
            t,
            root: cx.entity(),
            addr: self.den_join_addr.as_ref().expect("created above"),
            room: self.den_join_room.as_ref().expect("created above"),
            user: self.den_join_user.as_ref().expect("created above"),
            error: self.den_join_error.clone(),
        })
    }
}
