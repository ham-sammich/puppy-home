# PARITY.md — driving `redesign/gpui` to full parity with the egui app

The humans chose GPUI. This is the master checklist for the buildout; every
phase below works from this file. Update statuses in the same commit as the
work.

**Ground rules (in force until Phase G):**
- gpui sha **FROZEN** at Zed `v0.199.10` / `00789bf6ee744de8ddcfad93ade1d28cf4070a24`.
  No bumps mid-buildout; the bump is a Phase G decision.
- **No-divergence rule**: shared-layer changes (workspace/, backend/,
  session.rs, supervisor.rs, pack.rs, relay/) land frontend-agnostic and
  cherry-pick cleanly to `redesign/egui` / `redesign/shared-backend`.
- The `egui-shell` cargo feature stays compiling as the safety net until
  parity is declared; stripping it is a Phase G decision.
- Patterns: snapshots-down / `dispatch`-up, popover = `deferred` +
  `occlude` + click-out, all decorative motion gated on
  `Session.reduce_motion`, bounded render tails. See GPUI_NOTES.md.

Statuses: `[ ]` todo · `[~]` in progress · `[x]` done · `[n/a]` rejected/
not applicable · `[?]` needs a human decision first.

**Already at parity** (Phases 2.1-2.5, see REDESIGN_QA.md): dashboard
(cards/header/banner/views), workspace chat (multiline IME input, 4
composer skins, transcript + markdown subset + diff chips, slash palette,
explorer tree + changes list, ask-answer panel), the Den (full room), tab
navigation, toasts, reduce-motion, session prefs (view/style/motion).

---

## Phase B — daily-driver core

- [x] B1. Image paste: gpui clipboard PNG entries + shared arboard
      RGBA->PNG fallback; removable thumbnail chips (in-bar for Unified);
      sent via `send_user_prompt(text, images)`. (c5cf117)
- [x] B2. `@file`: '@ File' picker popover (Classic/Unified) inserting the
      egui-identical `@relative/path ` token; `@` typing still drives
      sidecar completions. Chips render as text tokens, not chip objects
      (same as egui's behavior — see its QA deviations).
- [x] B3. Input polish: soft wrap + multi-row cursor geometry, Up/Down
      across visual lines, Home/End + cmd-arrows, word jump (alt/ctrl,
      +shift select). PUNTED (cosmetic ledger): cursor blink, goal-column
      stickiness, double-click word select, internal scroll past 8 rows.
- [x] B4. Prompt-history navigation: Up/Down at top/bottom edge recalls
      via shared `history_prev/next` (draft stash, egui semantics).
- [x] B5. Palette keyboard nav: Up/Down wrap-around, Enter/Tab accept,
      Esc dismiss; palette-open routes keys away from buffer/history.
- [ ] B6. Sessions browser + resume: list/preview/load autosaves. Backend
      events already fold (`sessions`, `session_preview`, `load_session`).
      Ref: `workspace/sessions.rs`.
- [ ] B7. New chat flow: clear transcript + reset sidecar session (the
      egui `+ New chat`). Ref: egui `view.rs` new-chat + sessions plumbing.
- [ ] B8. Logs panel: collapsible sidecar-stderr view in the chat. Backend
      `Workspace::logs` exists (needs accessor). Ref: egui `chat_body.rs`.
- [ ] B9. Thinking auto-collapse: honor the one-shot `collapse` Cell when a
      turn completes (currently manual fold only). Ref:
      `workspace/render.rs` Thinking arm.
- [x] B10. Session restore on launch: egui semantics (missing dirs
      skipped), agent/model/autosave re-applied; saves are read-modify-
      write (egui layout/theme preserved) + change-gated in the drain
      loop; probe runs isolated from the user's session.json. Round-trip
      with an egui-written file proven live. (6ceed92)
- [x] B11. Composer dock turn controls: Pause/Resume + Stop + now/queue
      steer toggle in the status line while a turn runs, every skin;
      Enter mid-turn steers with the chosen mode. (c5cf117)
- [ ] B12. Markdown upgrade (ADDED): tables + links (open in browser) for
      the in-house renderer, or revisit the dependency decision. Ref:
      gpui_ui/markdown.rs decision note in GPUI_NOTES.md.
- [ ] B13. Triage bugs — *placeholder: user notes incoming.*

## Phase C — IDE surfaces

- [ ] C1. Editor tabs with syntect highlighting; click a tree file to open
      (tree rows are currently inert — ADDED note). syntect becomes a
      DIRECT dep (NOT via egui_extras). Ref: `workspace/editor.rs`,
      `workspace/state.rs` (`FileBuffer`, `EditorItem`), egui
      `editor_area.rs`.
- [ ] C2. File-tree A/M/D change markers + tree ops (new/rename/delete
      modals, context menu). Ref: `workspace/tree_ops.rs`, `git.rs`
      working-tree status plumbing (`git_changes` needs accessors).
- [ ] C3. Changes diff viewer (ADDED — was missing as its own item):
      per-file colored diff page; `DashAction::Changes` currently lands on
      a toast stub. Ref: `workspace/diff.rs` (`render_diffs`,
      `current_diff`).
- [ ] C4. Git view: staging list, commit box, push/pull/fetch + action
      feedback. Ref: `workspace/git_view.rs`, `git.rs`.
- [ ] C5. Git graph (all-branches commit DAG + branch dialog). Ref:
      `workspace/git_graph.rs` (pure layout) + `git_graph_view.rs` (paint).
- [ ] C6. Git credential prompts (HTTPS auth modal on push/pull/fetch).
      Ref: `workspace/git_creds.rs`.

## Phase D — terminal

- [ ] D1. Port the embedded terminal as a custom GPUI element: keep
      `terminal.rs`'s portable-pty + vt100 backend (PTY spawn, reader
      thread, `pump()`); build a GPUI Element that paints the vt100 grid
      (shaped mono runs + bg quads, cursor, scrollback) and routes
      keystrokes/IME via the input.rs patterns. Ref: `terminal.rs`,
      `theme/terminal.rs` palette.
- [n/a] D2. Zed `terminal`/`terminal_view` crates — REJECTED (dependency
      weight; pulls editor-stack crates). Decision recorded in
      GPUI_NOTES.md.
- [ ] D3. Slim terminal-mode bar (terminal/sessions toggles + agent/model
      switchers) when the terminal fills the chat area. Ref: egui
      `chat_body.rs` `render_bottom_bar`.

## Phase E — app management

- [ ] E1. MCP manager + wizard. Ref: `views/mcp_manager.rs`,
      `views/mcp_wizard/`. Backend events already fold (`mcp_servers`,
      generation counter).
- [ ] E2. Skills manager + wizard. Ref: `views/skills_manager.rs`,
      `views/skills_wizard.rs` (`skills`, `skill_detail` events).
- [ ] E3. Agent manager + wizard (JSON agent configs, visual builder,
      tool/MCP catalogs). Ref: `views/agent_manager.rs`,
      `views/agent_wizard/` (`agent_configs` events).
- [ ] E4. Remote SSH connect flow: connect dialog, off-thread spawn,
      `Supervisor::adopt`, remote-label UI states. Ref:
      `views/remote_connect.rs`, `app/remote.rs`, `backend/remote.rs`,
      `backend/ssh.rs`.
- [ ] E5. Path browser (shared folder-picker widget for wizards/remote).
      Ref: `views/path_browser.rs`.
- [ ] E6. Theme switching (Dark/Light/custom from themes.json) + the theme
      editor. Tokens currently hardcode `ThemePalette::dark()`. Ref:
      `theme/mod.rs` (`visuals_for` equivalent -> `Tokens::from_palette`
      already exists), `theme/editor.rs`.
- [ ] E7. Den leftovers: legacy Activity status broadcast +
      `.puppy/pack.json` Tier-2 breadcrumb sync (+ periodic re-stamp,
      removal on leave). Ref: `app/pack_sync.rs` (`sync_pack_breadcrumb`,
      `broadcast_pack_activity`).
- [ ] E8. Browser-plugin host tab: launch the plugin exe + stdin toolbar
      protocol; native window embedding stays per-OS (Windows reparent /
      macOS overlay). Ref: `browser/` (`mod.rs`, `host.rs`, `embed.rs`),
      `plugin.rs`.
- [ ] E9. Dashboard plugins section (ADDED): the collapsed installed-
      plugins list at the bottom of the egui dashboard. Ref: egui
      `dashboard/mod.rs` `plugins_section`.
- [ ] E10. Perf HUD equivalent: frame/notify counters overlay so the QA
      idle-discipline steps are verifiable in-app (egui had `perf.rs`).
- [?] E11. Dock/split layout (ADDED — DECISION NEEDED): the egui app has
      egui_dock split panes persisted via `Session.layout`/`SavedTab`; the
      gpui app is single-window tabs. Decide: accept tabs as the GPUI
      model (recommended; mark Session.layout egui-only) or build a
      pane-splitting system. Until decided, `Session.layout` must survive
      read-modify-write saves (it does — carry tested).

## Phase F — shared-base backend (cherry-picked both ways)

- [ ] F1. Sidecar ctx-% in the status payload -> card context-progress bar
      lights up on BOTH branches. Ref: `sidecar.py` `emit_status`,
      `workspace/events.rs` status arm.
- [ ] F2. Cost ledger investigation: can Code Puppy report per-turn $ cost?
      If yes -> `cost` field populates and the em-dash rule retires.
- [ ] F3. Keep the ask/steer/prompt seams identical across branches
      (standing rule; verify at each cherry-pick).

## Phase G — hardening + merge

- [ ] G1. Full perf/motion audit at parity scope (repeat of 2.5 across all
      new surfaces; every `with_animation` site gated, every list bounded).
- [ ] G2. REDESIGN_QA.md rewritten to parity scope (terminal step becomes
      real; managers/git/editor steps added).
- [ ] G3. **WINDOWS SMOKE GATE (required before merge)**: gpui pin must
      build + run on Windows (DirectX backend; `runtime_shaders` is a
      macOS-only concern). App smoke: open folder, prompt, terminal, den.
      ConPTY path of terminal.rs re-validated under the GPUI element.
- [ ] G4. Sha-bump decision: stay at v0.199.10 or bump to current Zed
      stable; budget 1-2 days for API chase if bumping.
- [ ] G5. egui-shell strip decision: delete the feature + egui-coupled
      modules + eframe deps, or keep one release as a fallback toggle.
- [ ] G6. Merge to master (after G3 passes and the humans sign off).

## Cosmetic ledger (small, known, deliberate — fix opportunistically)

- Flex-grid last-row stretch (Grid view; CSS-grid would not stretch).
- Avatar ring pulses instead of spinning (no cheap rotation at this pin;
  revisit after G4 bump).
- Plan cards cap at 8 checklist rows (no in-card scroll).
- Emoji: gpui renders color (feature, not bug) — egui branch stays mono;
  note in any side-by-side screenshots.
- Input cursor does not blink (static caret).
- Input: no goal-column stickiness on Up/Down, no double-click word
  select, content past 8 visual rows clips (no internal scroll).
- @file completions/picker insert text tokens, not chip objects (egui
  behaves the same; "chips" upgrade would be both-branch work).
- Kanban card hover-state element ids can collide on equal dir-name
  lengths (cosmetic only; relay ids authoritative).
- Den teammate read-along (Open on teammates' agents) disabled on BOTH
  branches — parity-neutral; future protocol work, not a gpui gap.
- Composer placeholder says "enter sends, shift-enter newline" in prose —
  restyle as the mock's key-glyph footer once B3 lands.
- Attention-banner question truncates to one line (hover shows nothing —
  add tooltip when convenient).

---

*Cross-checked against the egui branch's `src/views/` + `src/workspace/`
module trees on 2026-06-12. Items marked ADDED were not in the approved
phase list but exist in the egui app and not in `gpui_ui/`.*
