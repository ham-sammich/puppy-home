# GPUI branch notes (`redesign/gpui`)

Working notes for the GPUI rebuild. Forked from `redesign/shared-backend`
(`0f00eed`), **not** from `redesign/egui`.

## The pin (bumped to v1.6.3 in Phase G4, 2026-06-15)

```toml
gpui = { git = "https://github.com/zed-industries/zed",
         rev = "601ecb3ee5c16940191818ee7f244837abf6983c" }
gpui_platform = { git = "https://github.com/zed-industries/zed",
         rev = "601ecb3ee5c16940191818ee7f244837abf6983c",
         features = ["runtime_shaders", "wayland", "x11"] }
```

- **rev** `601ecb3ee5c16940191818ee7f244837abf6983c` = tag **v1.6.3**, the
  newest *stable* (non `-pre`) Zed release at bump time (2026-06-15).
  Verified: `cargo build`/`test`/`clippy`/`fmt` green on macOS arm64
  (rustc 1.96.0); probe-run launches, renders chat/terminal/MCP overlay,
  sidecar reaches Ready. Bumped from the old freeze (`00789bf` = v0.199.10)
  for freshness; Zed crossed 1.0 in between, so this jumped 0.199 -> 1.6.
- **ARCHITECTURE CHANGE:** the OS platform backends were split out of the
  core `gpui` crate into **`gpui_platform`** (+ `gpui_macos`/`_windows`/
  `_linux`). The app entry is now
  `Application::with_platform(gpui_platform::current_platform(false))` —
  `Application::new()` no longer exists.
- **`runtime_shaders` MOVED** from `gpui` to `gpui_platform` (→ `gpui_macos`).
  Same purpose: this machine has the Xcode CLT only (no `xcrun metal`), so
  shaders compile at runtime. `wayland`/`x11` are enabled on `gpui_platform`
  to keep the Linux CI leg building (inert on mac/windows).
- gpui is now crate **version 0.2.x** (was 0.1.0). Zed migrated `objc` ->
  `objc2` internally over this span (our own `objc` dep for the macOS
  browser-embed `embed_mac.rs` is independent and unaffected).
- The GPUI API churns between revs. Every signature in `src/gpui_ui/` was
  re-checked against the checkout at the new rev
  (`~/.cargo/git/checkouts/zed-*/601ecb3/crates/gpui`).
  **The Zed source at the pin is the documentation.**

### v0.199.10 -> v1.6.3 API delta (what moved)
- `Application::new()` -> `Application::with_platform(gpui_platform::current_platform(false))`.
- `Window::focus(handle)` -> `focus(handle, &mut App)` (gained the cx arg).
- `Line::paint(origin, line_height, window, cx)` -> `paint(origin,
  line_height, TextAlign, Option<Pixels> align_width, window, cx)`.
- `ScrollHandle::max_offset()` now returns `Point<Pixels>` (was a Size):
  `.height` -> `.y`.
- `flex_grow()`/`flex_shrink()` now take an `f32`; use `flex_grow_1()`/
  `flex_shrink_1()` for the old grow:1/shrink:1 behavior.
- `PathPromptOptions` gained a `prompt: Option<SharedString>` field.
- `ClipboardEntry` gained an `ExternalPaths(_)` variant (matches must cover).
- `Entity::update` returns the closure's value; `let _ = entity.update(..)`
  on a unit-returning closure now trips clippy (drop the `let _ =`).

## What compiles, what's parked

Strategy: **replacement, pragmatically**. The egui-coupled modules stay in
the module tree and keep compiling (so reusable logic inside them — e.g. the
`Workspace` god-struct's event folding, the git/diff state, the theme
palette — can be extracted incrementally), but nothing runs them by default.

- `eframe`/`egui_*` stay in `Cargo.toml` **temporarily** as type providers
  for the parked modules. The default binary never executes eframe code.
- Cargo feature **`egui-shell`** (off by default):
  - off → `main()` runs `gpui_ui::run()` (the GPUI app); all egui-coupled
    modules (`app`, `views`, `shell`, `workspace` UI, `theme`, `terminal`,
    `dock_layout`, `fonts`, `perf`, `session`, `browser`, `pack`, `git`,
    `plugin`, `proc`) are `allow(dead_code)` via `cfg_attr` so the build
    signal stays clean.
  - on → `main()` runs the legacy eframe app, full lints restored. Escape
    hatch while extraction is in flight; delete once the port is complete.
- **Nothing was deleted.** All pre-existing tests (workspace, theme, relay,
  backend, protocol, dock…) still compile and run — `cargo test` covers the
  same set as on `redesign/shared-backend`, plus the new `gpui_ui` tests.
- New deps: `gpui` (+ its tree), `futures` (drain-loop select; already in
  the tree transitively), `anyhow` (gpui's `AssetSource` trait speaks
  `anyhow::Result`).

## Design tokens — one truth

`src/gpui_ui/tokens.rs` does **not** hard-code the brand. `Tokens::dark()`
parses the hex strings out of the shared `ThemePalette::dark()` amber preset
(the exact same values the egui branch renders), so the two redesign branches
cannot drift. The single exception is `Tokens.bg` (`#121217`, the app
backdrop behind the panels) — the palette has no equivalent field because
egui's outermost fill *is* `panel`; it keeps the GPUI_GUIDE constant and is
documented at the definition.

Fonts: Space Grotesk (UI), JetBrains Mono (numbers/paths), Noto Emoji
(fallback) — the same OFL binaries in `assets/`, embedded via an
`AssetSource` impl (`src/gpui_ui/assets.rs`) and registered with
`cx.text_system().add_fonts(...)` at startup.

## The waker + drain-loop pattern (template for every later task)

GPUI is retained/reactive — there is no per-frame `update()`. Two pieces
bridge our threaded backend into that world:

1. **`GpuiWaker`** (`src/gpui_ui/waker.rs`) implements the shared `UiWaker`
   trait. GPUI's foreground executor is `!Send`, so backend threads cannot
   touch entities; instead `wake()` pushes onto an unbounded
   `futures::channel::mpsc` — cheap, lock-free, any-thread, idempotent in
   effect because the receiver coalesces.

2. **The drain loop** (`RootView::spawn_drain_loop`): one foreground task
   spawned from the root entity's `Context`:

   ```text
   loop {
       this.update(cx, |root, cx| { root.supervisor.drain(); cx.notify(); })?;
       cadence = any_busy ? 250ms : 1s
       select_biased! {
           _ = wake_rx.next()  => {}   // backend event → drain immediately
           _ = timer(cadence)  => {}   // floor: status polls, elapsed timers
       }
       drain wake_rx backlog            // N wakes → 1 drain (coalesce)
   }
   ```

   - Wakes give **event-driven latency** (a sidecar message renders on the
     next loop turn, not at the poll boundary).
   - The timer is the **cadence floor**: `Workspace::poll_status` only
     *issues* status requests from inside `drain()`, so something must call
     drain periodically while a turn runs — 250 ms busy, 1 s idle.
   - `this.update(...)` returning `Err` means the root entity is gone
     (shutdown) → the loop exits; no leaked task.
   - Later views follow the same shape: hold state in an entity, mutate in
     `update`, `cx.notify()`, never poll from `render`.

3. **Probe mode** (scaffold instrumentation): `PUPPY_GPUI_PROBE=1` logs a
   fleet summary line to stderr whenever it changes
   (`name: status tok=N rate=R/s [status_line]`), `PUPPY_GPUI_OPEN=/path`
   auto-opens a workspace at startup, and `PUPPY_GPUI_PROMPT="..."` fires a
   one-shot prompt at the first ready sidecar — together they prove the live
   plumbing end-to-end without clicking around. The prompt goes through
   `Workspace::send_user_prompt` (new seam in `workspace/chat.rs`), the same
   frontend-agnostic entry the GPUI composer will use — NOT a raw
   `backend.send_prompt`, which would bypass the `running` flag and so never
   arm status polling.

### Shared-backend fix found while proving the plumbing

`workspace/events.rs`: on `UiEvent::Result` the workspace now requests one
final status snapshot. Provider usage lands at turn end — *after* the last
in-flight poll was answered — and polling stops with `running`, so both
frontends previously showed a stale token total until the *next* turn.
Observed: `idle tok=0` forever → now `idle tok=24879` lands ~1s after the
turn completes.

### Observed live (probe transcript, macOS arm64)

```text
[probe] puppy-home: starting tok=0 rate=0.0/s [Starting Code Puppy…]
[probe] sending prompt to puppy-home: "Run `echo woof` via shell and report the output."
[probe] puppy-home: running tok=0 rate=0.0/s [Ready · code-puppy · claude-code-claude-fable-5-long]
[probe] puppy-home: tool tok=0 rate=0.0/s [Ready · code-puppy · claude-code-claude-fable-5-long]
[probe] puppy-home: idle tok=0 rate=0.0/s [Ready · code-puppy · claude-code-claude-fable-5-long]
[probe] puppy-home: idle tok=24879 rate=0.0/s [Ready · code-puppy · claude-code-claude-fable-5-long]
```

A real sidecar (auto-provisioned via `uv run --with code-puppy`), a real
shell-tool turn, statuses and the token total ticking through
waker → drain → `cx.notify()`.

## Build & test numbers (Task 2.1, macOS arm64, rustc 1.96.0)

- Clean debug build (gpui + 515 deps): **53s wall / 4m56s CPU**, 26 MB binary.
- Release build: **1m29s wall / 8m05s CPU**, **5.7 MB** binary (lto=thin +
  strip; the dormant eframe code is linker-GC'd — the egui branch's release
  binary was 16 MB). Release binary probe-verified: window opens, runtime
  Metal shaders compile, sidecar reaches `idle` live.
- Warm incremental rebuild: ~1s.
- `cargo test --workspace`: **169 passing / 0 failed** (149 bin — includes
  2 new `gpui_ui::tokens` tests — + 12 dock + 6 relay e2e + misc), zero
  skipped: nothing was cfg'd out of the test build.
- Known third-party noise: `block v0.1.6` future-incompat report (pulled by
  the pre-existing `objc` macOS FFI, not by gpui; same on the base branch).

## Task 2.2 — Dashboard architecture (the template for 2.3+)

### Entity / view structure
ONE gpui entity: `RootView`. It owns the `Supervisor` and every piece of
dashboard UI state (view mode, toasts, the open inline input, the open model
popover, the focus handle, pending navigation intent). Everything below it is
**stateless render code**:

```text
RootView (Entity, owns Supervisor + UI state)
 ├─ toolbar (brand · puppy chip · Open Folder · motion toggle · segmented)
 ├─ dashboard::pack_header / attention_banner   (header.rs — plain fns)
 ├─ dashboard::fleet ─┬─ card::AgentCard         (RenderOnce, snapshot-fed)
 │                   ├─ model_pill (deferred popover)
 │                   └─ table::FleetTable       (RenderOnce, List view)
 └─ widgets::toast_layer (absolute, bottom-center)
```

### State flow: snapshots down, actions up
`render` never hands live `&Workspace` references to components. It builds
plain `CardSnapshot` structs (strings + numbers + a cloned 40-float spark
ring) once per frame and moves them into `RenderOnce` components. Costly
extras (the model catalog) are only snapshotted for the card whose popover
is open. Benefits: components are `'static` (no borrow fights with gpui's
closure-heavy API), trivially testable, and the live data has exactly one
reader path.

Interactions all flow through ONE funnel: handlers capture
`Entity<RootView>` and call `root.update(cx, |r, cx| r.dispatch(DashAction::X, cx))`.
`dispatch` is the only place that mutates workspaces (pause/resume/stop/
steer/send/set-model via the shared `Workspace` card-action senders ported
from redesign/egui), pushes toasts, persists prefs, and records nav intents.
It is the moral equivalent of the egui branch's `ShellAction` queue — same
vocabulary, same backend calls.

### Popovers, toasts, inputs
- **Popover** (model switch): state = `RootView.model_popover:
  Option<WorkspaceId>`. Rendered inside the pill's `relative()` wrapper as an
  `absolute()` panel wrapped in `deferred(…).with_priority(100)` so it paints
  above sibling cards; `.occlude()` + `.on_mouse_down_out(…)` close it on
  outside click.
- **Toasts**: `Vec<Toast>` on the root, pruned by the drain loop (which runs
  fast while toasts are alive — `busy || !toasts.is_empty()`), rendered as an
  absolute bottom-center layer. Every dispatch arm pushes one.
- **Inline inputs** (steer / new prompt): one `CardInput` at a time on the
  root; a focusable div (`track_focus`) handles keys — printable chars via
  `Keystroke::key_char`, backspace, cmd-V paste, Enter submits through
  `dispatch(SubmitInput)`, Escape closes. Deliberately minimal: **the full
  IME-aware `EntityInputHandler` input (gpui's `input.rs` example, ~700
  lines) is the 2.3 composer's job.** Don't grow this one.

### Animation & reduce-motion
`with_animation(id, Animation::new(…).repeat().with_easing(ease_in_out), …)`
drives: status-dot halo pulse (1.6s), live avatar ring glow (3.4s), card
entrance fade (one-shot, keyed by workspace id so it plays once per card).
EVERY decorative loop is gated on the shared `Session.reduce_motion` flag
(same session.json field as redesign/egui; toggle in the toolbar persists
read-modify-write so the egui branch's fields survive). Reduce-motion swaps
pulses for static rings — state stays legible without motion.

### Sparklines
No chart primitive: `canvas(…)` + `gpui::Path`. The painter fills a soft
area under the curve plus a 1.4px offset band for the line — two
`paint_path` calls, no per-frame allocation beyond the cloned samples.
Used at 104×18 (header Throughput, fed by `Supervisor::aggregate_sparks`)
and 46×16 (card tok/s, fed by `Workspace::spark_history`).

### Status vocabulary (parity with redesign/egui)
Starting→"Waking up" · Running→"Fetching" · Thinking→"Sniffing" ·
ToolCalling→"Digging" · Waiting→"Needs you" · Paused→"Napping" ·
Idle→"Resting" · Dead→"Stuck". Sort rank: waiting → live → paused/stuck →
resting. Spend prints "—" while the cost ledger is absent — never $0.00.

### Known 2.2 gaps (deliberate)
- `Open →` / `Changes` / `Answer →` record a `NavIntent` + toast (the chat
  and diff views land in 2.3; the intent enum is already consumed there).
- No context-progress bar (sidecar still lacks ctx%; same gap as egui).
- Grid uses flex-wrap `minmax(420,1fr)`-style sizing; last-row cards may
  stretch wider than a CSS grid would — acceptable, looks intentional.
- Inline input has no cursor movement / selection / IME (see above).
- Visual QA was log-based this session (no screen-recording permission for
  `screencapture`); animations confirmed by code-path, not by eyeball.

## Task 2.3 — Workspace Chat decisions

### Markdown: in-house minimal renderer (`gpui_ui/markdown.rs`)
Evaluated Zed's `markdown` crate at the pin: it depends on `language`
(tree-sitter + the whole syntax stack), `theme`, `ui`, `sum_tree`,
`workspace-hack` — adopting it means swallowing half the editor for a chat
transcript. **Decision: ~250-line in-house subset** (headings, bullet lists,
inline `code`, **bold**, fenced code blocks with language tag; everything
else renders as plain text), unit-tested. Revisit only if real transcripts
demand tables/links.

### Terminal: deferred — "Terminal: egui branch only" (option c)
Neither porting the vt100 grid (a) nor Zed's terminal crates (b) fit this
task's budget; the composer input was the schedule risk and won the time.
The comparison note: the egui branch HAS the embedded terminal; GPUI does
not (the Classic composer skin says so in-UI). If/when needed, option (a) —
porting our own `terminal.rs` vt100 grid as a custom-painted Element — is
the planned route; we own that code and the Element API (see input.rs) has
everything required (shaped runs + paint_quads).

### The composer input (`gpui_ui/input.rs`) — the 2.2 deferral, paid off
Full `EntityInputHandler` port of gpui's `examples/input.rs` at the pin:
IME (marked text + underline), cursor, selection (mouse drag + shift-arrows
+ cmd-A), clipboard, character palette. **Extended to multiline**: content
keeps real `\n`s; each line is shaped separately (`Vec<ShapedLine>` + line
start offsets); cursor/selection quads are per-line; mouse maps row-by-y →
column-by-x. Enter emits `Submitted`, shift-enter inserts a newline; key
bindings are registered once under the `"ChatInput"` key context.
Deliberate gaps: **no soft wrap** (long lines clip), no up/down cursor
movement, no cursor blink. `send_user_prompt` converged to the egui 2-arg
`(text, images)` superset per the sync note — both branches now share the
exact prompt path.

### Chat architecture (same shape as 2.2)
`RootView.screen: Option<WorkspaceId>` routes Dashboard vs Chat; the tab
strip (Dashboard + per-workspace tabs with status dots + close) drives it
through the same `dispatch(DashAction)` funnel (`actions.rs` — split out of
`mod.rs` for size, same impl). Per-workspace `Entity<ChatInput>`s are
created lazily on first open; subscriptions translate `Edited` →
`Workspace::update_completions` (the egui composer's exact debounce) and
`Submitted` → send-or-steer (Enter steers while a turn runs). Transcript
renders a **120-entry tail** (egui parity) inside a `flex_col_reverse`
column — children are built newest-first, which pins the scroll to the
bottom with zero scroll-anchoring code. Diff bodies parse **lazily** (only
while expanded, capped at 200 rows). Slash palette = sidecar completions
(click to apply; `apply_completion` honors prompt_toolkit's caret-relative
`start_position`). All four composer skins (Classic/Unified/Palette/Guided)
are chrome around the ONE ChatInput entity; the gear popover persists
`Session.composer_style` (same serde field as redesign/egui).

### Task 2.3 parity gaps (honest list)
- Interactive asks (`ask_user_question`) have **no answer UI** in GPUI yet —
  the egui branch's pending-prompt modal didn't make the cut. Waiting cards
  + banner still surface the question text.
- No image paste / attachments in the composer (egui has clipboard PNG).
- No `+ New chat`, no sessions browser, no logs panel, no git view.
- Explorer: lazy tree + Changes list shipped; **no A/M/D change markers**
  (needs the private git_changes plumbing) and files don't open (no editor).
- Thinking entries: manual fold toggle; the turn-end auto-collapse one-shot
  is ignored.
- No soft wrap in the input (above).

## Phase E run 3 — pack sync, browser host, perf HUD

- **Pack sync rides the drain loop.** `pack_sync_upkeep()` (activity
  broadcast + Tier-2 breadcrumb) runs per drain tick; every behavior is
  self-rate-gated (2s activity / 300s re-stamp), so the 250-1000ms drain
  cadence is safely inside egui's per-frame calls. The breadcrumb body
  builder moved INTO `DenState` (`breadcrumb_body`) — frontend-agnostic,
  unit-tested against the egui JSON shape; DenState folds Activity pings
  + Claims now.
- **Browser = manager API, not manager render.** `BrowserManager` grew a
  frontend-agnostic surface (PluginStatus/NavOp/navigate_to/...) so the
  GPUI shell never touches tabs directly; egui's render methods keep
  mutating them in place. One `Screen::Browser` surface, lazy tab, URL
  bar is a ChatInput whose `Submitted` event funnels into the action
  dispatch. Embedding is N/A in the GPUI shell on every OS at this pin
  (Windows reparent = egui HWND, macOS overlay = eframe inner_rect); the
  webview floats in its own OS window and the viewport note owns it.
- **Perf HUD measures what the shell can see**: element-tree build time
  in `RootView::render` (`frame_begin`/`frame_end` bracket the build) +
  renders/sec. GPUI's layout/paint happens after the entity update, so
  it's invisible from here — the HUD labels say "render build" and the
  footnote owns the difference. Toggle: click the toolbar fleet-stats
  text. The drain loop's unconditional `cx.notify()` keeps the numbers
  ticking at 1-4Hz while idle.
- **Probes**: `PUPPY_GPUI_BROWSER=1`, `PUPPY_GPUI_PERF=1`.

## Phase E run 2 — remote connect + theming

- **Tokens re-resolution (the theming spine).** `RootView.tokens =
  Tokens::from_palette(&palette_for(theme, library))` on every pick/edit;
  render-side everything follows via the snapshot pattern. Long-lived
  `ChatInput` entities can't read the root, so two seams cover them:
  `Tokens::set_current()` (a root-written static `ChatInput::new` reads —
  the ONE sanctioned global, documented in tokens.rs) and an
  `apply_palette` walk pushing `set_tokens` into every live input.
  `bg`/`dim` stopped being constants: they're palette fields now
  (`app_bg`/`dim_text`, serde-defaulted so legacy themes.json loads).
- **Theme editor = one input pool + per-keystroke read-back.** 45 pooled
  inputs (name + 25 palette + 3 term + 16 ANSI; `T_*` indices in
  theme_ui.rs), seeded on open/load/start-from; every `Edited` event
  re-reads ALL fields into the working buffers and live-applies (cheap:
  hex strings). `palette_slots()` owns the (label, field) pairing once —
  seeding, read-back and rendering all iterate it (a missed field is a
  test failure, not a silent gap). Editing implicitly selects
  `Theme::Custom(name)`, exactly egui's `changed` outcome.
- **Terminal palette live-applies** to the running terminal:
  `term_colors = TermColors::from_theme(buffer)` on each edit; Save
  writes terminal.json (egui parity is the per-frame ctx-data insert).
- **Remote connect rides the waker.** Listing + connection both run on
  plain threads that `waker.wake()` when done; `remote_upkeep()` in the
  drain loop polls the receivers (egui's per-frame `try_recv` +
  `poll_remote`, verbatim — including keep-dialog-open-on-error and
  ignore-dismiss-while-connecting). The blocking `ls` body is shared with
  egui (`remote_connect::list_remote_blocking`). Adoption =
  `Supervisor::adopt` + jump to the new workspace's chat.
- **Probes**: `PUPPY_GPUI_THEME=dark|light|<name>` (picks + opens the
  editor), `PUPPY_GPUI_REMOTE=1` (opens the dialog).

## Phase E run 1 — manager overlay patterns (MCP / Skills / Agents)

- **One overlay, one field pool.** `manager_open: Option<MgrKind>` gates a
  single centered overlay (sessions-browser pattern: `deferred` + `occlude`
  scrim, priority 210). Because only one manager is open at a time, a small
  POOL of `ChatInput` entities (`mgr_inputs`, indices `F_*` in
  `managers.rs`) is reused across every form/wizard: fields are **seeded
  when a form opens** and **read back on advance/submit** — no
  per-keystroke field sync, no per-form entity churn.
- **Shared state machines, GPUI dispatch.** The egui wizards'
  frontend-agnostic structs (`views/{mcp,skills,agent}_wizard::Wizard` —
  paste parse/validate, review compose, scope mapping) are driven directly
  by `dispatch_mcp/skills/agents`; their private fields were widened to
  `pub(crate)` (sync-queued for the egui branch — behavior unchanged).
  Step gating (validate-before-advance) matches egui exactly.
- **Paste mode = the editor input.** One shared code-mode `ChatInput`
  (`mgr_paste_input`) carries every paste buffer, re-highlighted per edit
  via `editor::highlight` with the grammar keyed off the open manager
  (`x.json` vs `SKILL.md`).
- **egui cadence mechanics ported 1:1**: serving-workspace invariant
  (first ready sidecar), request gap 2s / mcp refresh 5s / slow refresh
  10s, optimistic toggle overrides (`mgr_pending`) cleared when the
  catalog generation bumps, generation bump re-fetches the open detail.
  `mgr_upkeep()` rides the existing drain loop — no new timers.
- **Probe**: `PUPPY_GPUI_MGR=mcp|skills|agents` opens the overlay once a
  sidecar is ready (render-survival validation, same style as the
  terminal/chat probes).

## Phase D — the terminal element

- **Split**: terminal.rs keeps PTY/vt100/reader-thread (renderer-free
  surface: with_screen/send_bytes/scroll_lines/resize_to/size) + the ONE
  key table (`named_key_seq` by gpui key names + `ctrl_byte`) and the
  shared `ansi_cube`; the egui `ui()` painter stays for the egui-shell
  feature, its key_seq now an adapter over the shared table.
- **Painting**: snapshot the vt100 grid per render (cells coalesced into
  attribute runs per row), paint in ONE canvas: one shaped line per row
  with multi-color TextRuns, bg/underline quads via x_for_index, block
  cursor (filled focused / outlined not). vt100 0.16 exposes no damage
  API; full-grid snapshot+shape per render is bounded by the visible grid
  (<= ~50 rows) and renders only happen on waker/drain notify. The reader
  thread wake is throttled to 8ms (output floods like `yes` cap at
  ~120 wakes/s with gentle PTY backpressure).
- **Resize**: the canvas measures cell box + bounds, records wanted
  rows/cols into a shared slot; the root applies it at the START of the
  next render (elements can't mutate entities mid-paint; one-frame lag).
- **Keys**: raw `on_key_down` in a "Terminal" key context (no bindings =
  nothing swallows Tab/arrows/Esc): shared table -> escape sequences,
  ctrl chords -> control bytes, printables via key_char, cmd-V paste.
  No mouse selection-copy, no mouse reporting (egui has neither).
- Theme: terminal.json (shared file) resolved to gpui colors at startup.

## Phase C run 1 — editor patterns

- **Code-mode input**: same ChatInput entity, `soft_wrap=false` — width =
  widest shaped line (parent scrolls both axes), Enter inserts newline,
  no row cap. One input implementation, two modes.
- **Layout cache**: entity-held `(generation, wrap_px) -> Arc<WrapLayout>`
  (RefCell, single-threaded interior mutability). Editors re-render on
  every drain notify; without the cache a whole-file reshape would run at
  4Hz. Generation bumps on content/syntax change only.
- **Highlight pipeline**: syntect (direct dep, the egui_extras pin) runs
  once per edit in the root's Edited handler -> per-line `(len, color)`
  SyntaxRuns -> consumed by the cached shaper. 200KB cap. IME marked-text
  underline is skipped while syntax runs are active (punt).
- **Tree context ops**: right-click -> inline panel at the top of the
  explorer (not a positioned popover — simpler, keyboard-free), same
  shared perform_rename/perform_new/delete_path ops as egui's modals.
- Extractions queued for sync batch: tree_ops fn visibility (pub(crate)
  delete_path/perform_rename/perform_new), Workspace::save_file/
  set_file_content/file_view + editor-tab/changes accessors, EditorItem +
  language_for re-exports.

## Phase B — composer patterns (B10/B1/B11/B5)

- **Soft-wrap input**: `shape_text(wrap_width)` per logical line →
  `WrappedLine` (multi-row aware `position_for_index`/`index_for_position`
  do all caret/mouse geometry); element height via
  `request_measured_layout` (shape in the measure closure, cap at 8 rows).
  Selection = one quad per visual row (start/end x from caret positions,
  full width between). Punts: goal-column stickiness, internal scroll
  beyond the cap, cursor blink.
- **Key routing precedence** (input actions): palette open → palette
  events (Nav/Accept/Dismiss); else Up/Down at the top/bottom visual row →
  History events; else cursor movement. The `palette_open` flag is pushed
  onto the input entity by the root (on edits AND on drain-loop completion
  replies — the sidecar answers async).
- **History recall** lives on `Workspace` (shared `history_prev/next` +
  `suppress_completions_for`); suppress is called BEFORE `set_text` so the
  deferred `Edited` event equality-debounces in `update_completions`.
- **Image paste**: gpui `ClipboardEntry::Image` (PNG straight through) with
  the shared arboard RGBA→PNG fallback; pending images live on the root
  per workspace as `(base64, Arc<gpui::Image>)` — wire form + thumbnail
  form built once at paste time.
- **Session restore/save**: probe runs (`PUPPY_GPUI_OPEN`) neither restore
  nor write session.json; real runs restore with egui semantics
  (missing dirs skipped) and save read-modify-write (egui-only fields
  preserved), change-gated in the drain loop.

## Task 2.4 — Needs-you answers + The Den

### Answer UI (the 2.3 carry-over)
One answer pipeline, both frontends: `ask_submit`/`ask_cancel` were
extracted out of the egui modal into frontend-agnostic `Workspace` methods
(the egui render path now calls them), plus accessors/mutators for both
blocking shapes — `ask_user_question` (`ask_state` / `ask_toggle_option` /
`ask_set_other`) and plain input/confirm/select prompts (`pending_request` /
`pending_choose` / `pending_answer_text`). The GPUI side renders an answer
panel between transcript and composer (wait-pink frame): option chips with
radio/checkbox semantics, a free-text Other row sharing ONE `ChatInput`,
Submit/Cancel; input prompts get a text row (Enter sends); confirm/select
are click-to-answer. Dashboard `Answer \u{2192}` routes to the chat where the
panel sits above the composer.

### Den architecture
- **`DenConn`** on the root: `PackClient` + event receiver + the shared
  `DenState` mirror (`src/pack.rs`, untouched) + locally-derived per-
  `(user, dir)` tok/s `SparkRing`s fed by successive roster broadcasts
  (pruned when members leave — bounded memory).
- **Pump + broadcasts ride the drain loop**: `pump_den()` folds relay
  events each tick, then (a) roster broadcast every 2.5s, **change-gated**
  (`format!("{agents:?}")` signature compare — exact egui `pack_sync`
  parity), (b) presence = window unfocused (`window.is_window_active()`
  captured in render) OR >5min since the last `dispatch` — **sent only on
  state flips**. PackClient's reader thread wakes the UI through the same
  `GpuiWaker`, so relay traffic renders event-driven.
- **Screen routing** grew an enum: `Screen::{Dashboard, Chat(id), Den}`;
  the Den tab appears in the strip while joined (green/red liveness dot,
  \u{2715} = Leave), and the toolbar button doubles as Join/Show.
- **UI**: join form (3 ChatInputs, relay defaulting to `$PUPPY_RELAY` or
  `127.0.0.1:9220`), header (motion-gated 1.6s LIVE blink, room-code chip
  click=copy+toast, "N people \u{b7} M puppies \u{b7} working {project} together"
  where {project} = most-common roster dir, mono relay host, Invite copies
  a shareable line, Leave), Roster/Board segmented, roster member groups
  (owner-colored avatars, you/host tags, presence dots, compact RoomAgent
  cards with sparkline + Open-own / Nudge\u{2192}`puppy_msg`), Board = shared-
  plans strip (checklist parse `- [ ]`/`- [x]`, struck done rows, Share
  picker over open workspace roots containing plans.md, Unshare) above the
  4-column kanban (\u{ff0b} add per column, \u{22ef} menu \u{2192} Move/Assign-me/Unassign/
  Retitle/Delete via typed ops — no drag-drop, like egui), feed (340px,
  human/puppy-with-review-badge/system, owner colors, `flex_col_reverse`
  bottom-pin, 150-entry tail + show older, ChatInput composer).
- Probe: `PUPPY_GPUI_DEN=addr,room,user` joins at startup; the probe line
  logs `den[room alive members roster feed tasks plans]` counts.

### Den gaps vs the egui branch (honest)
- No legacy `Activity` status broadcast (the egui app also feeds the old
  member-list strings); GPUI sends roster + presence only.
- No `.puppy/pack.json` breadcrumb sync (Tier-2 "[pack context]" prompt
  injection) — egui-only for now; the relay protocol side is identical.
- Plan cards render the first 8 checklist rows (no scroll-within-card).
- Kanban id collisions possible in element ids if two cards share a dir
  length (cosmetic hover-state only; relay ids are authoritative).

## Status (Task 2.1)

- [x] Branch `redesign/gpui` forked from `0f00eed`.
- [x] gpui pinned + building (see pin above).
- [x] Window "Code Puppy", `#121217` backdrop, tokens from the shared
      palette, bundled fonts registered.
- [x] `GpuiWaker` + adaptive drain loop driving `Supervisor`.
- [x] "Open Folder…" via gpui's native `prompt_for_paths`; bare-bones live
      workspace rows (status dot/label, dir, total tokens + tok/s).
- [ ] Dashboard / Command Center (Task 2.2 — do NOT start before the
      scaffold is reviewed).

Known scaffold gaps (deliberate): no workspace close button, no chat, the
row list is unstyled-on-purpose, `cx.activate` brings the app forward on
every launch.
