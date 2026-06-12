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
      SYNC QUEUE — SYNCED (shared-backend b5b6516 / egui d1a0c16): pub(crate)
      visibility opens on `views/{mcp_wizard,skills_wizard,agent_wizard}`
      state machines + `views/{agent,skills}_manager` helpers (egui
      behavior unchanged — fields/methods only widened so the GPUI
      dispatch drives the same state machines).
- [x] E4. Remote SSH connect: "Connect Remote…" in the toolbar (egui's
      top-bar slot) -> centered dialog with ~/.ssh/config hosts list,
      `[user@]host[:port]` target + remote-path fields, remote folder
      browser, inline errors, "Connecting over SSH…" replaces the buttons
      while the worker runs (egui behavior incl. blocked dismissal).
      Worker = the same `CodePuppy::spawn_remote` -> `Supervisor::adopt`
      flow; success jumps to the workspace chat (egui pushes a Chat tab).
      Probe: PUPPY_GPUI_REMOTE=1 (live-validated). Real-host end-to-end
      connect still pending a reachable SSH box (flagged for QA).
- [x] E5. Path browser: the dir-pick listing panel inside the connect
      dialog (folders-first alphabetical, ".. up", mono cwd header,
      "(empty)", inline error, "Use this folder"). egui's second call
      site (file-pick mode backing the local @file picker) is already
      covered by the GPUI B2 picker — not duplicated. Loading shows a
      static "loading…" label, not a spinner (motion discipline).
- [x] E6. Theme switching + editor: toolbar `Theme: {label}` popover
      (Dark / Light / customs from themes.json / Edit themes…), live
      apply via `Tokens::from_palette` re-resolution + `set_tokens` push
      into every live ChatInput (`Tokens::current()` seam covers inputs
      created later); selection persists through the shared session.json
      (read-modify-write). Editor overlay = egui's window at parity:
      library load/New/Save/Delete, Start-from presets, dark-base toggle,
      per-field rows w/ live swatch + hex input + per-keystroke preview
      (edits implicitly select a Custom theme, egui's `changed`), terminal
      palette (fg/bg/cursor + 16 ANSI) w/ live apply to the running
      terminal + Save to terminal.json. `bg`/`dim` became palette fields
      (`app_bg`/`dim_text`, serde-defaulted — legacy themes.json loads).
      Deviations: saved-theme combo box -> flat chip row; egui's native
      color-picker button has no GPUI counterpart at this pin (hex fields
      are canonical in both editors). Probe: PUPPY_GPUI_THEME=light
      (live-validated).
      SYNC QUEUE — SYNCED (shared-backend b5b6516 / egui d1a0c16; egui's
      convergent palette_for kept, identical semantics): theme/mod.rs `app_bg` +
      `dim_text` palette fields + `palette_for` (visuals_for now wraps
      it); theme/editor.rs pub `upsert`/`unique_name`/`ANSI_NAMES` + the
      two new color rows; theme `save_terminal` re-export;
      views/remote_connect.rs `list_remote_blocking` extraction +
      pub(crate) `join_remote`/`parent_remote`/`ListResult` (+ their new
      unit tests).
- [x] E7. Den leftovers, both behaviors in `gpui_ui/den/pack_sync.rs`
      (drain-loop driven; cadences inside egui's 2s/2.5s/300s gates):
      activity broadcast (same "name: state (tool)" \u{b7}-joined string,
      change-gated) and the Tier-2 breadcrumb (write-on-change + 300s
      re-stamp + helper drop + removal once the den connection dies).
      DenState now folds Activity pings + Claims (additive shared
      change); `breadcrumb_body` lives on DenState with a byte-shape
      unit test against the egui output (incl. "status" bare-detail and
      puppy-chat decoration). egui keeps its own PackView copy until the
      sync batch converges it.
- [x] E8. Browser-plugin host: toolbar "\u{1f310} Web" -> Screen::Browser
      (strip shows a Web tab once opened). Install panel at egui parity
      (status per manifest state, Install-from-local-build, Open plugins
      folder, Rescan, dir path, errors); running surface = the stdin
      toolbar (back/forward/reload/DevTools/CDP-copy/URL bar w/ Enter
      nav-or-launch + normalization reflected back) over the same
      `BrowserHost` process supervision. Deviations: ONE surface (egui
      docks N tabs); explicit Stop button (egui kills via dock-tab
      close); EMBEDDING N/A IN THE GPUI SHELL ON ALL OSes at this pin —
      the Windows reparent targets the egui HWND and the macOS overlay
      glues to the eframe viewport; neither attaches to the GPUI window.
      The webview runs in its own OS window (both paths' pre-embed mode)
      and the viewport region says so. Probe: PUPPY_GPUI_BROWSER=1
      (live-validated; plugin-not-installed path).
- [x] E9. Dashboard plugins section: collapsible "Plugins (n)" under the
      pack header (egui default-open, same status colors ready/
      incompatible/exe-missing, dir tooltip, version).
- [x] E10. Perf HUD: top-right overlay toggled by clicking the toolbar
      fleet-stats text (dev-obscure, egui's menu-item spirit). Maps:
      avg/max cost vs the 16.7ms budget (GPUI shell measures the
      element-tree BUILD in render — gpui layout/paint isn't visible to
      the shell; labeled honestly), renders/sec (drain-loop demand),
      memory rows (Windows API; zeros = hidden elsewhere, egui-same),
      uptime + the demand-not-cap footnote. Probe: PUPPY_GPUI_PERF=1
      (live-validated).
      SYNC QUEUE — SYNCED (shared-backend b5b6516 / egui d1a0c16; the
      Phase-D terminal.rs surface + workspace terminal accessors went in
      the same batch): pack.rs DenState
      activity/claims folds + `breadcrumb_body` + `PACK_HELPER` +
      `write/remove_pack_breadcrumb` (egui app/pack_sync.rs + PackView
      should converge onto them); browser/mod.rs frontend-agnostic API
      (PluginStatus/NavOp/stop_tab/nav/navigate_to/launch_tab/
      install_local/rescan/plugins_dir/open_plugins_folder/tab_running/
      tab_launch_error/install_error/local_build_available); perf.rs
      pub(crate) helpers (WINDOW/push/mean/peak/fmt_bytes/
      process_memory).
- [?] E11. Dock/split layout (ADDED — DECISION NEEDED): the egui app has
      egui_dock split panes persisted via `Session.layout`/`SavedTab`; the
      gpui app is single-window tabs. Decide: accept tabs as the GPUI
      model (recommended; mark Session.layout egui-only) or build a
      pane-splitting system. Until decided, `Session.layout` must survive
      read-modify-write saves (it does — carry tested).

## Phase F — shared-base backend (cherry-picked both ways)

- [x] F1. Sidecar ctx-% in the status payload -> card context-progress bar
      lights up on BOTH branches. Ref: `sidecar.py` `emit_status`,
      `workspace/events.rs` status arm.
      DONE: `ctx_pct` (0-100, one decimal, null = unknowable) delegates to
      the library's own /context plugin estimator
      (`context_indicator.usage.get_current_usage` — raw chars/2.5, immune
      to the token_ratio_learner monkeypatch, stable across model
      switches). Bar per design: 3px, gradient think->run, live cards
      only, tooltip with exact %; null draws nothing (a 0% bar would lie).
- [x] F2. Cost ledger investigation: can Code Puppy report per-turn $ cost?
      If yes -> `cost` field populates and the em-dash rule retires.
      VERDICT (option b): the library has NO cost ledger, but bundles a
      dated models.dev snapshot (`models_dev_api.json`, the same file its
      model browser uses offline). Sidecar now tracks input/output tokens
      separately and prices them against the snapshot (exact provider
      match, else cheapest input rate — resellers mark up); payload adds
      `cost_estimated: true` and the UIs render `\u2248$X.XX` (card cell,
      table column, Spend tile on both branches). Models absent from the
      snapshot (e.g. subscription `claude_code` ids) stay null -> em-dash
      survives where pricing would be fiction. The em-dash rule never
      fully retires by design.
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
