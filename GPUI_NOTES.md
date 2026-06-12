# GPUI branch notes (`redesign/gpui`)

Working notes for the GPUI rebuild. Forked from `redesign/shared-backend`
(`0f00eed`), **not** from `redesign/egui`.

## The pin (FROZEN — do not bump mid-build)

```toml
gpui = { git = "https://github.com/zed-industries/zed",
         rev = "00789bf6ee744de8ddcfad93ade1d28cf4070a24",
         features = ["runtime_shaders"] }
```

- **rev** `00789bf6ee744de8ddcfad93ade1d28cf4070a24` = tag **v0.199.10**,
  the most recent *stable* (non `-pre`) Zed release at pin time (2026-06-12).
  Chosen over `main` tip because a release tag has shipped to users —
  the closest thing to "verified builds" a git dep offers. Verified locally:
  `cargo build` green on macOS arm64, rustc 1.96.0.
- **`runtime_shaders`**: this machine has the Xcode CLT only — no `xcrun
  metal` — so gpui's default ahead-of-time Metal shader compile would fail
  at build time. The feature compiles shaders at runtime instead (gpui ships
  it for exactly this situation). If CI ever gets full Xcode, this can drop.
- The GPUI API churns between revs (the design handoff's GPUI_GUIDE warns
  about this). Every signature in `src/gpui_ui/` was checked against the
  checkout at this rev (`~/.cargo/git/checkouts/zed-*/00789bf/crates/gpui`).
  **The Zed source at the pin is the documentation.**

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
