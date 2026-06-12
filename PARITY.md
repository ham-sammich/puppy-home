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
- [x] B6. Sessions browser: filtered list + preview + resume-here, /resume
      sidecar flag opens it; no delete (egui has none — matched). (9cfec78)
- [x] B7. New chat: workspace-toolbar button over the /clear machinery,
      egui enabled-gate; per-entry UI state resets. (9cfec78)
- [x] B8. Logs panel: toggleable, mono, bottom-pinned, 200-line tail.
      (9cfec78)
- [x] B9. Thinking auto-collapse at turn end (egui's collapse-Cell consumed
      in the drain loop); folds default open while streaming; manual toggle
      wins. (9cfec78)
- [x] B10. Session restore on launch: egui semantics (missing dirs
      skipped), agent/model/autosave re-applied; saves are read-modify-
      write (egui layout/theme preserved) + change-gated in the drain
      loop; probe runs isolated from the user's session.json. Round-trip
      with an egui-written file proven live. (6ceed92)
- [x] B11. Composer dock turn controls: Pause/Resume + Stop + now/queue
      steer toggle in the status line while a turn runs, every skin;
      Enter mid-turn steers with the chosen mode. (c5cf117)
- [x] B12. Markdown upgrade: clickable links, tables, blockquotes,
      horizontal rules — in-house, unit-tested. Still absent: images,
      nested lists, ordered lists (ledger). (9cfec78)
- [ ] B13. Triage bugs — *placeholder: user notes incoming.*

## Phase C — IDE surfaces

- [x] C1. Editor tabs + syntect (DIRECT dep, 5.3.0/default-fancy = the
      egui_extras pin; one syntect in the lock). Tree click opens; code-
      mode input (no wrap, h-scroll), dirty marker, Cmd/Ctrl+S + Save,
      dirty-close confirm; generation-keyed layout cache + per-edit
      highlight (200KB cap). Blame toggle deferred to C-run-2 (git
      cluster). (9246dae)
- [x] C2. Tree A/M/D/R/? markers (ws.tree_markers, egui colors) +
      right-click context panel: new file/folder, rename, delete-confirm
      over the shared perform_* ops. Dir markers not aggregated (file-
      only — ledger). (9246dae)
- [x] C3. Changes viewer: editor-area Changes tab; list from git working
      tree (else Code-Puppy diffs), lazy per-click diff load, colored
      rows + op/path/+A-D header; wired from dashboard card + explorer.
      (9246dae)
- [x] C4. Git view: branch header (ahead/behind), Refresh/Fetch/Pull/
      Push, staging lists w/ per-file +/- and all-buttons, commit box at
      a CONSTANT height (31a6dcb principle), diff preview, history list/
      graph toggle, blame toggle in the editor bar. (a657e7f)
- [x] C5. Git graph: shared compute_graph + per-row canvas painter
      (bezier-band edges, rounded-quad nodes, 8 lanes colors, 200-commit
      bound); click = commit patch tab; right-click action panel
      (checkout/merge/new-branch/cherry-pick/revert/reset-hard).
      NOT ported: delete-branch + soft-reset menu items (ledger).
      (a657e7f)
- [x] C6. Git creds: auth-failure modal over the shared submit/retry
      flow (username + token, error line, Retry/Cancel). Password shows
      plaintext while typed — no masking in our input yet (ledger).
      (a657e7f)

## Phase D — terminal

- [x] D1. Terminal as a GPUI canvas surface: vt100 grid with coalesced
      color runs, fg/bg/inverse/underline (egui's exact attribute set),
      block cursor, wheel scrollback + banner, shared key table + ctrl
      chords + paste, theme from terminal.json, resize via paint-measured
      slot, 8ms reader wake throttle. Live-validated: real zsh ran `ls`
      in-grid. No selection-copy / mouse reporting (egui has neither).
- [n/a] D2. Zed `terminal`/`terminal_view` crates — REJECTED (dependency
      weight; pulls editor-stack crates). Decision recorded in
      GPUI_NOTES.md.
- [x] D3. Terminal toggles in the workspace toolbar + Classic/Unified
      composer skins; terminal fills the chat area (egui placement). The
      dedicated slim bar is NOT replicated 1:1 — our toolbar + composer
      dock already carry its controls (sessions/agent/model/terminal);
      noted as a layout deviation, not a capability gap.

## Phase E — app management

- [x] E1. MCP manager + wizard: status-dot list (state colors, error tip,
      summary, optimistic enable switch), 3-step add wizard (transport
      cards / details / review) + Form|Paste toggle reusing the shared
      `mcp_wizard::Wizard` state machine (paste parse, mcpServers unwrap,
      transport inference, validate). egui has no test-connection action
      beyond wizard validation — matched exactly (Add/Refresh/toggles).
- [x] E2. Skills manager + wizard: user+project list w/ filter + enable
      toggles, detail pane (Edit gated on fetched detail), 3-step wizard
      + SKILL.md paste mode over `skills_wizard::Wizard`.
- [x] E3. Agent manager + wizard: list w/ filter + source badges +
      (active) marker, detail pane (Clone always; Edit/Delete gated on
      editable; delete blocked on the active agent, inline confirm),
      4-step builder (basics/prompt/tools+MCP chips/review) + JSON paste
      mode over `agent_wizard::Wizard`. Paste buffers are ONE shared
      code-mode input with live syntect highlighting (JSON for MCP/agents,
      markdown for skills) — the "if cheap" upgrade landed, not plain mono.
      Phase-E manager deviations (all three, deliberate):
      - Overlay (sessions-browser pattern, one at a time) instead of
        egui's dockable tabs; access is app-wide from the dashboard
        toolbar = egui's top-bar buttons. Serving-workspace invariant,
        poll cadences (2s gap/5s mcp/10s slow) and generation-driven
        optimistic-toggle clearing ported 1:1.
      - env/headers edit as KEY=VALUE lines, not add/remove pair rows.
      SYNC QUEUE (phase-end batch to shared-backend + egui): pub(crate)
      visibility opens on `views/{mcp_wizard,skills_wizard,agent_wizard}`
      state machines + `views/{agent,skills}_manager` helpers (egui
      behavior unchanged — fields/methods only widened so the GPUI
      dispatch drives the same state machines).
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
- Git creds password field is plaintext while typed (egui masks).
- Graph menu: delete-branch + soft-reset not ported.
- Commit box height fixed (96px), not drag-resizable like egui.
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
