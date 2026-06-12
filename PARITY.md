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

- [x] G1. Full perf/motion audit at parity scope (repeat of 2.5 across all
      new surfaces; every `with_animation` site gated, every list bounded).
      DONE @ ba196a8: 6/6 animations gated, all drain upkeeps gated, all
      rings capped, subprocess ledger bounded. TWO FIXES: uv update now
      kill-bounded at 5min (pipe-drain threads to dodge the 64KB pipe
      deadlock); browser embed RAF gated on window_active (no more vsync
      spin while unfocused). Leftover QW8 sync ALSO LANDED: avatar fields
      on shared-backend 244336e + egui 4325471 (egui renders the saved
      avatars via a OnceLock pair; no egui picker by choice).
- [x] G2. REDESIGN_QA.md rewritten to parity scope (terminal step becomes
      real; managers/git/editor steps added). DONE: 30-step ~20-min pass
      across 10 surfaces incl. den hosting, agent-creator, avatars,
      /cmds+ctx, version/update; stats refreshed (clean release 1m53s,
      12 MB binary, 970 deps unchanged, 223 tests, +29,675/-393 over 69
      commits); deviations appendix updated (old composer items obsolete).
- [x] G-DRIFT. The long-flagged egui<->shared divergence reconciliation
      (session.rs / supervisor.rs / shell / app). Hunk inventory across
      the trio: ~6 shared-logic drifts, ~12 frontend-specific, 1 dead
      duplicate, 0 rot in shared's app/ (fn-name intersection with
      workspace/ = only `new`). RECONCILED to shared-backend:
      dashboard_view/composer_style/reduce_motion Session fields +
      ComposerStyle/DashboardViewMode enums (canonical there, identical
      copies on both UI branches, docs say so); current_session now
      carries all prefs losslessly (avatar-carry pattern); chat.rs
      steer() deduped on shared+gpui to delegate to steer_text()
      (deletes the one dead duplicate — old inline body with literal
      emojis); egui's steer_text emoji literals re-escaped per repo
      rule. BOUNDED: egui-only avatars()+UiPrefs moved under an
      explicit EGUI-SHELL-ONLY banner in egui's session.rs.
      RESIDUAL-DRIFT LEDGER (deliberate, not debt):
      * #[allow(dead_code)] present only on shared — each branch keeps
        allows for accessors its UI doesn't consume (by design).
      * ShellAction (egui) vs gpui action funnel — parallel dispatch BY
        DESIGN; verified both are thin (handlers only call shared
        Workspace/Supervisor methods; no logic inside either).
      * shared's app/ + views/ + workspace render files = the frozen
        ORIGINAL egui UI, consumed by shared's own binary — not rot.
      * current_session signature: (theme) on shared vs (UiPrefs) on
        egui — egui-only aggregation; both carry non-owned fields.
      * egui's events.rs "asked:" note renders session::avatars() (QW8
        choice); shared/gpui keep the static \u{1f436} (frontend-spec).
      * submit()/steer() sit at different file positions on egui vs
        shared/gpui (egui hoisted them in its composer rework) — same
        bodies, placement-only.
      * Legacy-shell micro-delta accepted: steer() now optimistically
        bumps queued_steers on queued steers (egui's shipped behavior;
        next status poll corrects it anyway).
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

## B13 — first user test-drive (bugs fixed this run)

- [x] B13.1 remote-connect crash: remote_upkeep flipped Screen::Chat
      without ensure_chat_input -> render expect aborted the app. Fixed +
      class fix (render screen-sanity pass degrades instead of panicking;
      sibling unwrap audit: remote_pending take-first, terminal ctrl-chord
      guard). `9660d89`
- [x] B13.2 black-on-black/contrast: syntect highlight theme now follows
      the palette's dark flag (light themes got dark-theme pastels on
      light wells); theme switch re-highlights open code surfaces;
      tooltips (own render root, outside the text-color cascade) resolve
      Tokens::current() at show time instead of a stale dark constant.
      No bg-as-fg token uses found; root cascade rules out GPUI's
      default-black text. `6d1aaf9`
      NOTE: user said "some areas" without specifics — if it reproduces
      in DARK theme somewhere, these fixes may not cover it; need repro.
- [x] B13.3 model chip cut off: pill + wrapper min-w-0/flex-shrink so it
      ellipsizes inside tight header rows; full id on hover. `57dad96`
- [x] B13.4 bare /agent (+/model) now opens the GUI switcher popovers —
      sidecar intercepts before the CLI's prompt_toolkit menu (which
      blocks headless), re-emits the catalog with open:true; Workspace
      one-shots mirror wants_sessions. `bed06dd`
- [x] B13.5 /cd: sidecar announces cwd changes ({event:'cwd'}); workspace
      follows (root/title/tree/git rebind + transcript note). `6a70e81`
- [x] B13.6 Grid/List/Focus segmented control moved from the global
      toolbar into the dashboard body (right-aligned above the fleet).
      `075ae57`

- [x] B13.7 remote workspace terminal opened a LOCAL shell (BOTH shells —
      egui's spawn_terminal had the identical `Terminal::spawn(&self.root)`
      gap, no remote gating anywhere). Fixed in the shared layer:
      Workspace::spawn_shell picks local PTY shell vs interactive
      `ssh -t <dest> -- cd '<root>' && exec "${SHELL:-/bin/sh}" -l`
      (SshTarget::terminal_args: sidecar host-key/timeout conventions,
      port/identity flags, deliberately NO BatchMode — the PTY can take
      password/2FA prompts; ssh exit = the normal dead-shell notice).
      Root cause beyond the spawn: adopt only kept `user@host` for
      display — port/identity were dropped; Workspace now stores
      RemoteInfo { label, target } and both connect flows pass it
      through. VALIDATION: arg-shape unit tests + local-terminal probe
      green; real remote terminal needs a reachable host — flagged for
      human QA alongside E4. `2437bf0`

- [x] B13.8 remote workspace showed the LOCAL puppy identity.
      PROVISIONING VERIFIED SOUND: spawn_remote ships only our protocol
      shim (sidecar.py) to the remote cache and launches it THERE via
      `uv run --with code-puppy python` — code_puppy + ~/.code_puppy
      config (puppy name, agents, models, MCP) are all the REMOTE's.
      (Nuance: the default launcher resolves code-puppy from the
      remote's uv environment, not necessarily a remote pre-install;
      PUPPY_HOME_REMOTE_CP_CMD overrides.) The bug was identity
      plumbing: RootView::puppy_name() was first-reporter-wins across
      ALL workspaces, so a remote sidecar could become the app-global
      headline; and chat surfaces rendered the global name everywhere.
      FIX: headline pinned to LOCAL workspaces only (headline_puppy,
      unit-tested); chat who-lines/empty-state/Guided send/composer +
      sessions overlay speak the WORKSPACE'S own puppy (ws_puppy);
      dashboard cards lead the meta line with \u{1f436} {name} when a
      workspace's puppy differs from the headline (subtle, Den-spirit);
      Den roster already broadcast per-workspace names (verified).
      egui shell on THIS branch: chat is already per-workspace, no
      global heuristic exists — nothing to fix here; redesign/egui's
      own dashboard lede needs the same local-pin at sync time (queued).
      E2E vs a real remote needs human QA (standing E4 limitation).
      [RESOLVED during the fallback live E2E: vm840 runs — see the
      SSH-FALLBACK entry for the full live matrix incl. B13.7 terminal
      + push-creds-by-consequence.]
      `5df2868`

- [x] E8 REDUX #3 BROWSER POLISH after user feedback (dated look /
      can't pop back in / can't close):
      1. "DATED": wry's DEFAULT User-Agent is an anonymous WebKit string
         — google.com served its 2009 legacy no-JS homepage to it
         (beveled buttons, underlined links; screenshot-proven). Fix:
         with_user_agent matching real Safari 17.6 on macOS (Safari
         freezes the OS token at 10_15_7 — exact match is the point);
         Edge-shaped UA on Windows, Chrome-shaped on Linux. AFTER:
         modern Google (doodle, pill search, AI Mode). Crispness at 2x
         confirmed fine — scale theory ruled out, UA was everything.
      2. "CAN'T POP BACK IN" root cause: Float left the window glued at
         the embed rect, COVERING the host's browser toolbar — \u{2913}
         and Stop were unreachable (also half of "can't close"). Fix:
         Float now repositions to a standalone spot (logical 140,120 @
         1100x740). Host-window capture during pop-out shows the full
         toolbar incl. \u{2913}.
      3. "CAN'T CLOSE": the Web tab had NO close affordance — once
         opened, permanent. Fix: \u{2715} on the Web tab ->
         BrowserAction::CloseSurface (stop_tab + close_tab +
         browser_tab=None + leave Browser screen). Live: process GONE +
         all plugin windows gone from CGWindowList. Plugin death via
         window close (CloseRequested -> exit, code path present;
         approximated live with SIGTERM — can't click traffic lights
         headlessly): host flips to Launch immediately, no zombie
         toolbar. Toolbar Stop unchanged (kill -> overlay dies with
         process).
      Probes: PUPPY_GPUI_BROWSER=launch:<url> (UA canary),
      cycle probe stage 5 = close. Plugin rebuilt + reinstalled.
      Occluded-window captures via `screencapture -l <id>` (user was
      using the machine — full-screen shots were Discord). `968eb44`

- [x] E8 REDUX #2 GPUI IN-TAB BROWSER EMBEDDING (user: "should appear
      embedded with a pop-out icon"). The E8 "embedding N/A in GPUI"
      verdict is RETIRED — the investigation found the missing pieces
      exist at the pin (v0.199.10):
      HANDLE ACCESS: `impl HasWindowHandle for Window` (window.rs:4668)
      -> RawWindowHandle::AppKit{ns_view} on macOS (platform/mac/
      window.rs:1292), Win32{hwnd} on Windows. From the NSView, plain
      objc gives [view window] -> windowNumber (embed's z-anchor),
      isMiniaturized, and the full Cocoa rect chain.
      GEOMETRY: browser/embed_mac.rs converts the canvas-recorded
      element rect (window coords, logical) via convertRect:toView:nil
      -> convertRectToScreen: -> primary-screen y-flip ->
      backingScaleFactor, yielding the global top-left PHYSICAL px tao
      consumes — deliberately NOT gpui's Window::bounds() (its origin
      is current-screen-relative; breaks multi-display). Slot pattern =
      terminal ResizeSlot (canvas records at layout, render-start
      upkeep consumes, one-frame lag).
      BEHAVIOR: default EMBEDDED (borderless overlay glued to the
      Browser screen viewport; embed re-sent per render like the egui
      per-frame pump + request_animation_frame keeps frames flowing);
      screen-switch/minimize hides (drain-side isMiniaturized check +
      700ms wake ticker, since renders stop when miniaturized);
      \u{2197} pops out to the decorated float (plugin `embed` now sheds
      decorations so pop-in works — plugin REBUILD+REINSTALL needed);
      \u{2913} re-embeds. Mode is per-launch (default embedded; no
      persistence — YAGNI).
      LIVE VALIDATED (CGWindowList + screenshots): embedded rect glued
      inside the host at exact paddings; dashboard switch -> overlay
      offscreen; return -> re-embedded same rect; pop-out -> decorated
      'Puppy Browser' window; pop-in -> borderless in-tab again; quit
      -> plugin exits. Window drag/resize tracking is the same
      per-frame recompute the egui shell uses (not separately
      drag-tested — headless box).
      WINDOWS: by construction, untested — reparent-once into the GPUI
      HWND + place at client rect (embed_tab_win mirrors the egui
      pump); pop-out adds embed.rs `unparent` (SetParent NULL +
      WS_POPUP restore). Flagged for Windows validation pass.
      PUPPY_GPUI_BROWSER_CYCLE=1 staged probe drives the whole cycle
      headlessly. `1c4209e`

- [x] E8 REDUX browser "does nothing" on macOS in the GPUI shell (user:
      "opens one but there's no browser"). ROOT CAUSE: on embeddable
      platforms (macOS + Windows) the plugin window starts BORDERLESS +
      HIDDEN (`with_visible(false)`) awaiting the host's `embed` — the
      egui shell pumps embed every frame, the GPUI shell sends NOTHING,
      so the process ran with an invisible window while the UI claimed
      "running in a separate window". Verified live: CGWindowList showed
      the window onscreen:no pre-fix; an `embed` over stdin flipped it
      onscreen:1 (mechanism proof). FIX: new plugin `float` command
      (decorations on + visible = a real floating window); host
      BrowserHost::float(); BrowserManager::float_pump() sends it once
      per launched process when ready — called from the GPUI drain loop
      only (egui keeps embedding, never floats; protocol is
      backward/forward compatible — old plugins log-and-ignore float,
      new plugins under egui never receive it). NOTE: requires a plugin
      REBUILD + reinstall (Install from local build) — stale installed
      plugins predate the float command. E2E ON THIS MACHINE: app launch
      -> probe (PUPPY_GPUI_BROWSER=launch, probe extended) -> decorated
      "Puppy Browser" window onscreen rendering example.com
      (screenshot-verified), navigate over stdin OK, plugin exits with
      the app. Windows GPUI shell gets the same fix by construction
      (also starts hidden there) — untestable here, flagged. `d6f8017`

- [x] B13.2 REDUX input fields rendered BLACK text in dark mode (user
      report; the earlier sweep fixed surface styling + syntect code
      mode but missed the plain-text machinery). ROOT CAUSE: ChatInput
      shapes its own runs, and shaped runs don't inherit the div
      cascade's text_color — `cached_layout` colored plain content from
      `window.text_style().color`, which falls back to gpui's DEFAULT
      (black) whenever the surface container didn't set an explicit
      text_color. Every plain input without one went black-on-dark.
      SECONDARY: the shape cache key is (generation, wrap) only, so
      set_tokens kept stale-palette runs until the next edit. FIX at
      the machinery: plain runs (and the syntax fallback + marked-text
      path) color from the input's OWN tokens (self.tokens.text, pushed
      by apply_palette on theme switch); set_tokens now drops the cache.
      Placeholder (dim), cursor (accent), selection (accent 0.25) were
      already token-colored. ONE EntityInputHandler impl = every input
      surface fixed in one place: chat composer, answer input, sessions
      filter, git commit/branch/creds, tree rename/new, editor (syntect
      mode untouched), remote target/path, den join/message/card-title,
      browser URL, MCP/Skills/Agent wizard fields, theme-editor hex
      fields. No surface overrides run colors locally (containers'
      text_color only ever styled labels). Probed dark + light themes.
      `68e7a00`

- [x] B13.3 REDUX model chip still clipped (user screenshot): the previous
      fix left the 180px max-width ALWAYS on, so wide cards with free
      header space still truncated long ids. Now: no fixed cap — the pill
      is content-sized (full id whenever the row has room) and ellipsizes
      only when genuinely tight; a 62%-of-row fractional max on the row
      child wrapper (fractions don't resolve on the auto-sized inner pill)
      keeps pathological ids from squeezing the title to nothing. Focus
      view shares the card, so it inherits the fix. List view keeps its
      fixed table column by design (already ellipsized, no bleed) and
      gains a full-id hover tooltip. Verify vs the longest real id at
      grid/focus/narrow widths = human QA (visual). `2253af5`

- [x] FEATURE "puppush" built in (user's ~/.code_puppy/puppush script,
      generalized to all OAuth providers): push local code-puppy auth +
      model config to a remote host's ~/.code_puppy. Manifest derived
      from the code_puppy SOURCE (config.py + auth plugins), defined once
      in backend/creds_push.rs: SENSITIVE chmod 600 (claude_code_oauth,
      chatgpt_oauth, copilot_session, copilot_device_tokens .json) +
      plain model config (models, extra_models, claude/chatgpt/copilot/
      gemini_models .json). EXCLUDED deliberately: puppy.cfg (remote
      keeps its identity per B13.8 — AND plain API keys ride in it, so
      API-key providers are NOT covered; documented), mcp_servers.json
      (machine-specific), agents/skills/contexts/caches/history/terminal
      sessions (machine state). Transfer = sidecar-provisioning
      convention: per-file `mkdir -p && cat > file` over ssh stdin,
      BatchMode=yes (no PTY → fail fast), contents never logged. UI:
      Connect-Remote dialog button ("Push my auth + models to this
      host…", two-step confirm, works pre-connect) + "push creds" in the
      remote workspace's chat toolbar (two-step confirm; summary toast +
      per-file transcript note). Local-dir resolution mirrors
      code_puppy's XDG rule (env set → XDG, else ~/.code_puppy); the
      REMOTE side targets legacy ~/.code_puppy only (XDG-configured
      remotes not handled — noted). Unit tests: manifest classification,
      XDG mirror, command shape, summary. Live push vs the human's now
      working remote = human QA. `2253af5`

- [x] FEATURE SSH-FALLBACK MODE (user: "if the remote host doesn't
      support code puppy — use our local code puppy to send commands via
      ssh as a fallback").
      DETECTION: spawn_remote now preflights after provisioning succeeds
      (auth + POSIX shell proven): `command -v <launcher argv0>` over
      ssh. Exit 1 -> RemoteError::CannotHost (fallback OFFERED, never
      automatic); exit 255 (ssh-level) or any provisioning failure ->
      RemoteError::Other (plain error — wrong creds can never silently
      switch modes). Previously a uv-less host "connected" then died
      moments later; now it's caught before launch.
      APPROACHES: (a) SHIPPED — local sidecar in a scratch cwd
      (~/.cache/puppy-home/ssh-fallback/<slug>/) with a generated
      AGENTS.md; code_puppy natively loads ./AGENTS.md into the system
      prompt, so the instruction injection needs zero sidecar/protocol
      changes. (b) sshfs REJECTED: macFUSE install burden, no trivial
      detection. (c) code_puppy-native remote tooling REJECTED: source
      checked, no ssh/remote plugin exists.
      CAPABILITY MATRIX (honest):
        tree/editor          ssh-native SshFs (one-shot execs, CachedFs TTL)  REAL remote files
        git view/graph/stage ssh-native SshGit (GitRunner over ssh + macro)   REAL remote git
        terminal             B13.7 interactive ssh                            unchanged
        agent shell tool     works via `ssh target '...'` per instructions    model-dependent compliance
        agent file tools     LOCAL ONLY — instructed never to use them on     the honest gap;
                             project files; scratch cwd keeps strays penned   noted in-UI
        puppy identity       LOCAL puppy (it IS the local install);           headline-eligible
                             (B13.8 filter passes fallback workspaces)
      UI: dialog offer box on CannotHost (accent-framed, explicit
      Connect-in-fallback / Cancel); card meta gains "· ssh-fallback";
      chat toolbar badge w/ capability tooltip; transcript note on
      connect spelling out the limitation; toast labels the mode.
      Sidecar-RPC RemoteFs/RemoteGit were NOT reusable for this (they
      serve the sidecar host's disk = local in fallback) — that's WHY
      SshFs/SshGit exist. CachedFs generalized to Box<dyn WorkspaceFs>.
      egui safety net: maps RemoteError to text, no offer flow (queued
      for redesign/egui along with the rest).
      LIVE E2E DONE (vm840:/storage/weinsteinjcc.yobo.dev, Debian, NFS
      storage): forced CannotHost via PUPPY_HOME_REMOTE_CP_CMD override
      -> offer -> accept -> fallback workspace: tree == ssh ls ground
      truth; CHANGES(32) == remote git status; terminal on root@vm840
      in project cwd; AGENT listed remote-only files (.claude/,
      .well-known/, .nfs*) and identified the WordPress project — the
      local puppy answered from REMOTE content via ssh; ssh-fallback
      badge + push-creds btn + local identity all visible. Error paths
      live-verified: bogus host -> ssh resolve error (no offer), bogus
      path -> 'remote path ... doesn't exist' (no adopt, no offer).
      NORMAL remote mode also live: fallback=false, `uv run --with
      code-puppy` sidecar RUNNING ON vm840 (pgrep-verified), prompt +
      thinking + errors streamed over ssh stdio, sidecar died cleanly
      with the app (stdin EOF). A model-side flake on the remote
      code-puppy (output-validation retries) surfaced as a transcript
      error — transport innocent, rendering correct. Push-creds
      verified by consequence: ~/.code_puppy/claude_code_oauth.json on
      vm840 at 600, mtime DURING the run (the pushed creds were
      authenticating that very sidecar). Remote left clean.
      BUGS FOUND+FIXED by the live run (separate commits): rc-file
      function shadowing (`test()` on vm840 printed an IP, exit 0 ->
      exists() always true; fixed w/ `command` prefix, live #[ignore]
      regression test `cargo test ssh_fallback_live -- --ignored`);
      preflight path validation (bad path used to fake-connect then
      die); provisioning errors now surface ssh's stderr reason.
      Still unverified: real auth-failure variant (only DNS-failure
      tested; same code path), Windows-side ssh quoting.
      `34978da`

      SYNC QUEUE — SYNCED (shared-backend ac772f0 / egui 853a3a8; the
      full Phase E/F batch). Shared layer copied wholesale; divergent
      files (workspace/mod+chat+view, supervisor, app/remote) went in
      as 3-way merges against the pre-sync shared base, egui idioms
      kept. egui ALSO got its two queued UI ports: the CannotHost
      SSH-fallback offer (explicit, never automatic) and the B13.8
      headline-puppy local pin on the dashboard lede. Deliberately NOT
      synced: gpui's removed allow(dead_code) annotations (each branch
      keeps allows for accessors its UI doesn't consume); gpui took
      back shared's annotated browser/mod.rs, embed_mac.rs,
      creds_push.rs, backend/mod.rs for byte-lockstep (allows are
      inert where consumed). Original queue text follows.
      sidecar/sidecar.py (picker
      intercepts + cwd event + open flags), backend/mod.rs (Wire/UiEvent
      Agents/Models open + Cwd), workspace/events.rs + mod.rs
      (show_agent_picker/show_model_picker one-shots, set_root).
      egui-side consumption of the new one-shots is egui UI work, not
      queued here.
      SSH-FALLBACK additions: backend/ssh.rs (launcher_probe_command,
      exec_command, exec_shell + tests), backend/ssh_fallback.rs (new,
      shared), backend/mod.rs (RemoteError, preflight, spawn_ssh_fallback),
      workspace/fs.rs (CachedFs over Box<dyn WorkspaceFs>),
      workspace/mod.rs (RemoteInfo.fallback, remote_fallback()).
      redesign/egui needs: RemoteInfo.fallback at its adopt site,
      RemoteError mapping, optionally its own offer UI.
      B13.7 additions: terminal.rs (spawn_cmd split + spawn_remote),
      backend/ssh.rs (terminal_args + tests), workspace/mod.rs
      (RemoteInfo, spawn_shell, remote_label()), workspace/view.rs +
      supervisor.rs (adopt signature) — NOTE supervisor.rs and the
      egui branch's own app/remote.rs are in the known-divergent set:
      the redesign/egui port of the RemoteInfo passthrough is a small
      manual patch at sync time, not a blind file copy. Chose to queue
      rather than sync immediately for that reason.

      DOCUMENTED GAPS: remote workspaces keep their root-bound ssh git
      runner after a remote /cd (tree/title follow; git rebind needs a
      remote git factory); terminal-cd tracking deferred (OSC7 needs
      shell-side integration, PTY-child cwd polling is per-OS FFI —
      libproc on macOS / PEB reads on Windows); remaining CLI TTY menus
      (/tutorial onboarding, uc/mcp-bind) still no-op headless.

## FEATURE BACKLOG — from the user's test-drive notes (no implementation
   yet; sequencing happens outside this ledger)

Version + updates
- [x] Show Code Puppy version; check for updates; run updates. (QW1)
      Toolbar v{cp_version} chip (wire already carried it) -> About
      panel: PyPI check via curl (offline-safe), Update = uv
      --refresh-package cache bust (the honest mechanism for our
      'uv run --with code-puppy' spawns) + 'restart workspaces to
      apply'. Both legs live-verified (PyPI 0.0.561 == uv resolution).

Dashboard
- [x] Whistle button: create a new code-puppy instance at the home dir.
      (QW2 — pack-header button beside the H1)
- [x] "New Chat" next to Open Folder (same home-dir spawn). (QW3 —
      toolbar, jumps straight into the new chat; shared
      DashAction::OpenHome{to_chat})
- [ ] Auth status (Claude/GPT/Copilot/any model with surfaceable auth)
      + re-auth methods.

Den
- [x] "Join Den" should also CREATE a den (run `cargo run -p puppy-relay`
      for the user); multi-den hosting/joining; self-host instructions.
      (QW6 — Host a Den on the join screen: binary-next-to-exe first,
      cargo-run dev fallback; auto-join + LAN share line + Stop hosting;
      PUPPY_RELAY_WATCH_PID watchdog so SIGKILLed apps can't orphan the
      relay; docs/DEN_HOSTING.md for servers. LEDGER: multi-den stays
      future — DenConn/PackClient are single-connection by design.)

Agents
- [x] "Create Agent with Agent Creator" button — spawns a session using
      code-puppy's agent-creator agent inside the agent builder.
      (QW7 — Agents-manager header button: fresh $HOME workspace +
      set_agent("agent-creator") down the spawn pipe, lands in chat.)

Managers
- [x] Manage code-puppy config. (QW5 — puppy.cfg settings list,
      line-level INI edits preserving comments/sections, secret keys
      masked+locked, priority ordering, works sidecar-less)
- [x] Manage Models (a manager like skills/agents/mcps). (QW4 —
      catalog + extra_models.json overlay editor w/ syntect JSON,
      set-active via set_model, custom-entry remove; OAuth file
      secrets never surfaced)

Identity / setup
- [x] User PFP + Puppy PFP (emoji defaults). (QW8 — toolbar identity
      chip -> Avatars panel: You/Puppy targets, 40-emoji grid + any-
      emoji input; persisted as session.json user_avatar/puppy_avatar.
      SYNC QUEUE: session.rs gained the two shared serde fields +
      dock_layout carry-over — port the egui pickers next sync batch.
      LEDGER: avatar-in-den-roster needs a relay protocol slot
      (RoomAgentInfo has none); not extending the wire for this.)
- [ ] Initial setup guide: install code-puppy if absent, run setup,
      tutorial, puppy name; theme select (dark/light/system/custom,
      possibly importing code-puppy's themes); models setup + auth;
      composer style choice.

Git
- [ ] Create PRs (can use /generate-pr-description); view PRs if possible.

Composer
- [x] Pop button (/pop command). (QW9 — in the /cmds status-line pill;
      /pop sends the exact typed path, /pop N seeds the input. /pop
      verified in source: pops last N MESSAGES, system prompt kept.)
- [x] Context size/usage/status + context-related commands. (QW9 —
      'ctx N%' chip when ctx_pct known (calm/warn/alarm; unknown draws
      nothing) + /cmds popover: /pop, /compact, /truncate N,
      /dump_context, /clear — the set code_puppy actually ships.)

More views
- [ ] Goals/Judges: goal panel, judges' reviews panel, manage judges +
      guided judge builder (like mcp/skills/agents wizards).
- [ ] Kennel management/view (/kennel).
- [ ] Ollama management (/ollama-setup).
- [ ] Code-puppy plugins management (/plugins).
- [ ] Wiggum view (/wiggum + related commands).

---

*Cross-checked against the egui branch's `src/views/` + `src/workspace/`
module trees on 2026-06-12. Items marked ADDED were not in the approved
phase list but exist in the egui app and not in `gpui_ui/`.*
