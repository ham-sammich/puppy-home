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
