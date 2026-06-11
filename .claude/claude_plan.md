# puppy-home — Progress & Plan

_A native (Rust/egui) IDE-style GUI around the real **Code Puppy** Python agent.
puppy-home is a GUI + process supervisor; the actual agent (every tool, model,
MCP server, skill) is the genuine `code-puppy` package, driven over a JSON
protocol via an embedded Python **sidecar**. Code Puppy's source is **never
modified** — only puppy-home and `sidecar/sidecar.py`._

Last updated: 2026-06-11 · Canonical plan (merged from `claude_plan.md` +
Rufus's `PLAN.md`). This is the single source of truth — don't fork it.
Reconciled with reality (browser plugin, git graph, perf HUD, release builds
had landed unrecorded) and extended with a new feature roadmap planned by
planning-agent-8a6233 with Jacob (see "New feature roadmap (2026-06-11)").

**2026-06-10:** Priority 0 landed — the 2,919-line `workspace.rs` is now an
11-file `src/workspace/` module (largest 507 lines). Mechanical move only, no
behaviour change; `cargo check` clean, `cargo test` 7/7 green.

---

## Architecture (one screen)

```
 puppy-home (Rust / egui + egui_dock)            one sidecar per workspace
 ┌──────────────────────────────────┐   JSON    ┌──────────────────────────┐
 │ Supervisor + WorkspaceMap         │ ◀──────▶  │ Code Puppy (folder A)    │
 │  DockArea: Dashboard │ Chat tabs  │ ◀──────▶  │ Code Puppy (folder B)    │
 └──────────────────────────────────┘           └──────────────────────────┘
```

- **Process-per-workspace**: Code Puppy's cwd is process-global (`os.chdir`), so
  each opened folder = its own sidecar process (own cwd / agent / model / session).
- `src/backend/mod.rs` — one-sidecar handle: provisioning, spawn, JSON protocol,
  `UiEvent`/`Wire` types. (713 lines — second-fattest; watch it.)
- `src/supervisor.rs` — owns all `Workspace`s, spawns sidecars, drains events.
- `src/workspace/` — per-workspace state + status FSM + the whole chat/IDE view,
  split by responsibility (was one 2,919-line file; see Priority 0, now DONE):
  - `mod.rs` (507) — `Workspace` struct + lifecycle + event ingest (`apply_event`/`on_message`)
  - `state.rs` (159) — `InstanceStatus`, `Entry`, `Pending`, `EditorItem`, `GitView` + pending parsing
  - `view.rs` (390) — chat-tab shell: top bar, tree+changes sidebar, editor tabs, transcript, bottom bar, terminal
  - `git_view.rs` (451) — Git page/commit render + working-tree polling + diff/changes plumbing
  - `composer.rs` (419) — input box, completion, commands menu, pending prompts, agent/model/name controls
  - `editor.rs` (286) — file buffers, save/dirty/reload, inline blame, `language_for`
  - `sessions.rs` (232) — sessions browser modal + preview
  - `render.rs` (170) — transcript-entry/markdown/file-tree rendering
  - `diff.rs` (160) — `DiffRecord`/`DiffLine` + diff parsing/render + markers
  - `ask.rs` (158) — `AskState`/`AskQ` + `ask_user_question` modal
  - `chat.rs` (111) — turn lifecycle + command dispatch (send/steer/pause/answer)
- `src/shell/mod.rs` — `Tab`, `TabViewer`, deferred `ShellAction`s.
- `src/views/dashboard.rs` — cross-workspace dashboard.
- `src/app.rs` — hosts the `egui_dock` DockArea; session restore + persist.
- `src/git.rs`, `src/terminal.rs` (492), `src/session.rs`, `src/fonts.rs`.
- `sidecar/sidecar.py` — embedded via `include_str!`; bridges Code Puppy's
  MessageBus + legacy queue to line-delimited JSON over stdio.

**Protocol** — GUI→sidecar ops: `prompt, cancel, command, complete, list_commands,
list_agents, list_models, set_agent, set_model, set_puppy_name, status, list_sessions,
load_session, preview_session, pause, resume, steer, respond_*, shutdown`. sidecar→GUI
events: `ready, message, commands, agents, models, completions, ask, result,
command_done, error, log, status, paused, sessions, session_loaded, session_preview`.

**Load-bearing invariant:** immediate-mode UI and the async agent **never touch
directly** — events flow IN over a channel, structural changes flow OUT as
deferred `ShellAction`s. Do not bypass this. Ever.

---

## Completed

### Shell & workspaces
- Multi-workspace dockable shell (egui_dock 0.19.1 / egui 0.34.3); Dashboard +
  per-folder Chat tabs; "Open Folder…" async picker.
- Dashboard: per-instance state·tool, elapsed·N-tools, attention banner +
  tab `●` / top-bar "⚠ N waiting"; live token rate, conversation stats, and
  **concurrent sub-agent rows** (via the `status` op).
- **Persist/restore workspaces & sessions** — open folders (+ agent, model, and
  Code Puppy autosave session) saved to per-OS `session.json`, reopened and
  **resumed** on next launch.

### Chat / agent control
- Markdown rendering (egui_commonmark) with syntax-highlighted code; broad
  Unicode + monochrome emoji fonts (per-OS system fonts loaded at runtime).
- Agent + model native dropdowns; smart Commands ▾ menu; CLI-style `/` and `@`
  autocomplete using Code Puppy's own completers.
- Interactive `ask_user_question` modal (TTY tool monkeypatched to the GUI).
- **Cancel / Pause / Resume / Steer** a running turn (drives Code Puppy's
  `PauseController` + steer history-processor; now/queue steer modes).
- **Stream the agent's THINKING** live into the chat (capturing streaming
  console) as a collapsible `💭 thinking…` block that auto-collapses on
  completion — so a watching user can pause/steer mid-turn.
- Composer layout: input row (Commands + box + Send/Steer/Pause/Stop) over a
  bottom menu bar (🖥 Terminal · 🐶 name · Agent ▾ · Model ▾).

### IDE
- File tree + lazy dirs; editable file tabs with **syntax highlighting**, Save/
  Ctrl+S, dirty markers, auto-reload on AI edits.
- **Git**: working-tree Changes panel + inline A/M/D tree markers; full **Git
  page** (branch + ahead/behind, staged/unstaged lists, stage/unstage/commit,
  history → per-commit patch); **inline blame** toggle in the editor.
- **Full PTY terminal** (`portable-pty` ConPTY/openpty + `vt100`): real shell
  grid with colors/cursor/TUIs, keyboard→PTY, scrollback, auto-resize.

### Sessions (Code Puppy integration)
- **🗂 Sessions browser** (bottom bar / `/resume`): searchable list of saved
  autosave + named-context sessions with a **read-only conversation preview**
  pane; **Resume this** loads a session into the workspace.
- Each workspace is tied to its autosave session and resumes it across launches.

### Your puppy
- The puppy's **name** (Code Puppy global `puppy_name`, e.g. "Rufus") shown on
  replies (`🐶 Rufus:`), composer hint, empty state; toolbar **🐶 name** button
  renames it.

### Cross-platform
- Targets Windows / macOS / Linux: per-OS shell + fonts, native PTY, no
  compile-time Windows paths.

### Landed 2026-06-10/11 (branch `feature/browser-plugin`, head `e5bfe9b`, 81 tests green)
- **Browser plugin (Architecture C)**: optional separately-installed wry
  companion exe, discovered via `src/plugin.rs` `PluginRegistry` (`plugin.json`
  manifests, semver host-compat), supervised like a sidecar, native-window
  overlay on the Browser tab; CDP breadcrumb at `{workspace}/.puppy/browser.json`
  injected as prompt context by the sidecar so the agent can attach to the
  in-app browser; self-cleaning on tab/workspace close.
- **Git commit graph (GitKraken-style)**: pure lane-layout algorithm in
  `workspace/git_graph.rs` (unit-tested) + rendering/interactions in
  `git_graph_view.rs`; right-click context menu (checkout, create/delete branch,
  cherry-pick, revert, reset soft/mixed/hard, merge branch-or-commit, copy hash)
  and header Fetch/Pull/Push.
- **Performance HUD** (`src/perf.rs`): frame cost avg/max vs 60fps budget,
  repaints/sec, memory (1Hz while visible), toggled from the top bar.
- **Theme module split** (`src/theme/{mod,editor,terminal}.rs`).
- **Image paste**: clipboard images attach to prompts as base64 PNG ->
  pydantic-ai `BinaryContent`.
- **Release builds**: `scripts/build-release.ps1` -> `dist/` standalone Windows
  build (thin-LTO, stripped, `windows_subsystem=windows`),
  `.github/workflows/release.yml` for macOS/tagged releases; console-flash fix
  via `src/proc.rs` `hide_console()` (`CREATE_NO_WINDOW`) applied at every
  `Command` spawn site.

---

## Critical fixes (recent)

- **Terminal blank / can't type** — answer ConPTY's DSR `ESC[6n` cursor-position
  query (conhost blocks on it); stable focus id + focus-lock filter for keys.
- **Conversations never autosaved** — the sidecar bypasses the CLI loop that
  triggers autosave; now autosaves after each turn.
- **400 `tool_result` without `tool_use`** — after each run the sidecar now
  `set_message_history(list(result.all_messages()))` (the CLI does this; we
  didn't) + prunes orphaned tool pairs before run/save/load.
- egui Grid **ID-clash** in chat — namespace each entry with `ui.push_id`.
- Sessions modal grew off-screen — clamp to screen + Esc-to-close.
- `/clear` now also clears the on-screen transcript; `[FileContentMessage]` hidden.

### Verification approach
GUI screenshots are unreliable in this environment (multiple windows / overlay),
so behavior is verified with **headless sidecar tests** (drive `sidecar.py` over
the protocol) + Rust unit/integration tests (`cargo test`: session round-trip,
vt100 grid parse, PTY plumbing, key/color maps). The app is launched via
`PUPPY_HOME_CP_CMD` pointing at the cloned `D:\dev\code_puppy\.venv`.

---

## 🐘 Priority 0 — Tame `src/workspace.rs` (2,919 lines) — DONE 2026-06-10

**DONE (2026-06-10).** Split into the 11-file `src/workspace/` module above
(largest 507 lines), mechanical move only, `cargo test` 7/7 green. Historical
context kept below. This was the **single biggest structural risk** — it was
~5x past the 600-line guardrail and held:

- ~15 structs/enums (`Entry`, `Pending`, `DiffRecord`, `FileBuffer`,
  `EditorItem`, `GitView`, `AskState`, …)
- **Two** giant `impl Workspace` blocks: lines **270–988** and **988–2567**
  (~2,300 lines of methods on one type)
- ~20 free functions at the bottom (diff parsing, markdown, render helpers)

### Split plan — by responsibility, NOT to hit a line count

Create `src/workspace/` and break along seams that already exist:

| New module               | What moves in                                                       |
| ------------------------ | ------------------------------------------------------------------- |
| `workspace/mod.rs`       | `Workspace` struct + lifecycle (open/restore/close, event ingest)   |
| `workspace/state.rs`     | `Entry`, `Pending`, `PendingKind`, `InstanceStatus` + transitions   |
| `workspace/chat.rs`      | Composer, send/cancel/steer, transcript state, command dispatch     |
| `workspace/editor.rs`    | `FileBuffer`, `EditorItem`, file tabs, save/dirty/reload-from-disk  |
| `workspace/diff.rs`      | `DiffRecord`/`DiffLine`, `parse_diff`/`parse_unified`, diff render  |
| `workspace/git_view.rs`  | `GitView` UI panel (keep shell-outs in top-level `git.rs`)          |
| `workspace/ask.rs`       | `AskState`/`AskQ` interactive-question modal                        |
| `workspace/render.rs`    | `render_entry`/`render_message`/`render_markdown`/`labelled`/help   |

**Rules (followed):** one mechanical move (no behavior change), `cargo check`
clean + `cargo test` green, channel/deferred-action invariant preserved. **Next
fattest files to eyeball:** `backend/mod.rs` (713) and `terminal.rs` (492).

_Note for future pups:_ the project's emoji guard strips emoji from agent file
writes, so the move was done with a throwaway Python script that copied
emoji-bearing method/fn bodies verbatim file-to-file (extract-by-name, brace/
indent matching) — never retyping the UI glyphs. Reuse that trick for
`backend/mod.rs`/`terminal.rs` if they hold emoji.

---

## Planned / roadmap

| Priority | Item | Notes |
|---|---|---|
| ~~**P0**~~ | ~~**Split `workspace.rs`**~~ DONE 2026-06-10 | Now `src/workspace/` (11 files, largest 507 lines). |
| High | **Stream the agent's response** live (not just thinking) | response currently renders only on `result`; would need to capture the response stream (termflow) or pydantic-ai event iteration |
| ~~High~~ | ~~**CI: fmt + clippy + test, cross-platform matrix**~~ DONE 2026-06-10 (`.github/workflows/ci.yml`; whole tree now `fmt --check` + `clippy -D warnings` clean) | enforce `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on win/mac/linux — *prove* the cross-platform claim on every push |
| ~~High~~ | ~~**Pure-fn + protocol-contract unit tests**~~ DONE 2026-06-10 (49 tests; inbound `Wire` events + outbound op builders both pinned — see `backend/protocol.rs`) | `parse_diff`, `parse_unified`, `short_session`, `truncate`, `language_for`, `tool_label` are pure → free coverage during the split; add serde round-trip tests pinning every protocol op/event shape (the Rust↔Python seam) |
| ~~Med~~ | ~~**Sidecar crash recovery**~~ DONE 2026-06-10 ("Restart Code Puppy" button on backend death; re-spawns + re-attaches the session via the existing `set_restore` path - live kill-test still needs a human) | if a sidecar dies mid-session, surface it + one-click restart that re-attaches the autosave session (persistence plumbing already exists) |
| ~~Med~~ | ~~**Output backpressure**~~ DONE 2026-06-10 (transcript ring-buffer cap @ 1500 + "trimmed" banner; terminal scrollback already bounded by vt100) | runaway tool/terminal output shouldn't lock the UI — cap/virtualize transcript + scrollback (ring buffer, "… N lines collapsed") |
| Med | **Color emoji** | egui rasterizes monochrome only; image-based via `egui-twemoji` or a glyph-image atlas |
| Med | **"Save as named context"** action | snapshot the current conversation to `CONTEXTS_DIR` under a name (uses `save_session`) |
| ~~Med~~ | ~~**Dock-layout persistence**~~ SUPERSEDED by **Phase D** (see "New feature roadmap (2026-06-11)") | persist the egui_dock split layout, not just the open set — now a prerequisite of the right-sidebar dock |
| Low | **Command palette (Ctrl+P)** | fuzzy switcher across workspaces / files / commands — ergonomics win as workspace count grows |
| ~~Low~~ | ~~**Theme toggle**~~ DONE 2026-06-10 (light/dark toggle in the top bar, persisted in `session.json`; "system" mode + custom accents not done) |
| Low | **Keyboard shortcuts** + `?` overlay | Ctrl+W close · Ctrl+Enter send · Ctrl+L focus composer · Ctrl+` terminal |
| Low | Session list niceties | preview-on-hover; group by date; delete a session |
| Low | Per-workspace unique autosave id | two sidecars opened in the same second share a timestamp id (Code Puppy's `auto_session_\d{8}_\d{6}` regex forbids a suffix) |
| Low | Terminal polish | copy/paste selection, font-size control, split terminal+chat |
| ~~Low~~ | ~~MCP & Skills panels~~ SUPERSEDED by **Phase C** (see "New feature roadmap (2026-06-11)") | surface Code Puppy's MCP servers / skills — absorbed into the GUI managers & visual builders phase |

> **Status note (2026-06-10):** every roadmap item implementable *and verifiable
> headlessly* is now done — P0 split, full protocol-contract tests (52 total),
> CI + a `clippy -D warnings`-clean tree, output backpressure, sidecar crash
> recovery, and the dark/light theme toggle. The items still open above all
> need **interactive on-screen verification** (live response streaming,
> dock-layout persistence — which also needs runtime workspace-id remapping —
> color emoji, command palette, more keyboard shortcuts, terminal polish,
> MCP/Skills panels) or are blocked externally (per-workspace autosave id:
> Code Puppy's `auto_session_\d{8}_\d{6}` regex forbids a suffix). The
> feature-idea backlog below stays YAGNI-gated until someone actually asks.
>
> **Image paste (2026-06-10, on request):** clipboard images (Ctrl+V while the
> composer is focused, or the "Image" button) attach as removable thumbnails and
> ride along with the next prompt as base64 PNGs (`prompt` op gains an optional
> `images` array). The sidecar decodes them into pydantic-ai `BinaryContent` and
> passes them via the supported `run_with_mcp(attachments=...)` path. Clipboard
> read uses `arboard`; PNG encode/base64 is pure + unit-tested. Needs a
> vision-capable model to verify the full round-trip on screen.

### Feature ideas (YAGNI-gated — only build on real demand)
- **Cross-workspace broadcast** — send one prompt to N selected workspaces.
- **OS notifications** — toast when a backgrounded workspace finishes / waits.
- **Hunk-level staging** in the Changes view (git.rs is file-level today).
- **Session export** — dump a workspace conversation to markdown/HTML.

---

## New feature roadmap (2026-06-11) — decided with Jacob

Planned with planning-agent-8a6233. Priority order: **C -> D -> A -> B**.

### Phase C — GUI managers & visual builders (first; small-to-medium, low risk)
Goal: surface code-puppy's agents/skills/MCP servers in the GUI. No core
code-puppy changes — everything via new sidecar protocol ops wrapping
code-puppy's existing managers.
- New sidecar ops (and matching Wire events + protocol-contract tests in
  `backend/protocol.rs`): `list_mcp_servers`, `set_mcp_enabled` (per-server
  on/off toggle), `add_mcp_server`, `list_skills` (with metadata), `get_skill`,
  `save_skill`, `list_agent_configs`, `get_agent_config`, `save_agent_config`,
  `delete_agent_config`.
- MCP Manager panel: list servers with status, on/off toggles, "Add MCP server"
  guided wizard (transport type, command/url, env vars, test-connection step).
- Skills Manager panel: searchable skill list, enable/disable if supported,
  view skill detail; "Create skill" guided wizard generating the skill file
  scaffold.
- Agent Manager panel: list agents, edit existing; "Visual agent builder"
  wizard (name, model, system prompt, tool selection, output settings) writing
  code-puppy JSON agent configs.
- Each manager is a dockable Tab (reuses shell Tab/TabViewer pattern).
  Estimated: 3-5 increments.

**STATUS 2026-06-11: Phase C DONE.** All three managers landed (MCP inc.1
b93e86d, Skills inc.2 e7291f5, Agent inc.3 — adds list/get/save/delete/
clone_agent_config ops + src/views/agent_manager.rs + agent_wizard.rs, top-bar
"Agents" button). Built-in agents are read-only + cloneable; JSON agents are
editable/deletable. 115 tests green, zero warnings/clippy, fmt clean. All
sidecar ops verified headlessly against real code-puppy. Project also now
builds + runs on Apple Silicon macOS (see .claude/HANDOFF.md).

Phase D is now also DONE (commits 3846e7e + d812e7a) and a paste-and-validate
mode shipped for all three managers (ed15f83). Next: Phase A (Remote SSH).

### Phase D — Right sidebar dock + layout persistence (small) — DONE
- DONE (3846e7e): persistent right-side dock zone that panels (MCP/Skills/Agent
  managers, perf HUD, future Puppy Pack chat) can be dragged into/out of via
  egui_dock. DONE (d812e7a): dock-layout persistence in session.json with
  workspace-path remapping on restore + a rect-free structural change signature
  (no idle file churn). See src/dock_layout.rs and .claude/HANDOFF.md.
- (original notes below, for reference)
- Persistent right-side dock zone that panels (MCP/Skills/Agent managers, perf
  HUD, future Puppy Pack chat) can be dragged into/out of via egui_dock.
- Land the existing "Dock-layout persistence" roadmap item (persist egui_dock
  split layout in `session.json`; needs runtime workspace-id remapping on
  restore). This is a prerequisite so the sidebar arrangement survives
  restarts. Estimated: 1-2 increments.

### Phase A — Remote SSH ("Remote Sidecar", full remote IDE)
Architecture decision: run `sidecar.py` ON the remote host and tunnel the
existing line-delimited JSON stdio protocol over SSH (the protocol design makes
this nearly free). Rejected alternatives: SFTP-mount with local agent (tools
would run on the wrong machine); remote-desktop approaches (heavy).
- Connection profiles: per-host config (host, user, auth, code-puppy location,
  ports to forward) stored in session/config; new "Open Remote Folder..." flow.
- Config strategy (both modes day one, per-profile toggle):
  - "Respect host": use the remote machine's own `~/.code_puppy` as-is.
  - "Bring my puppy": on connect, sync local agents/skills/config to a
    session-scoped remote dir (never clobbers the host's config) and point the
    sidecar at it via env.
  - Auto-provision code-puppy via uv on the remote if not installed (model on
    the existing local provisioning logic in `backend/mod.rs`).
- Full remote IDE from the start: introduce a `WorkspaceFs` trait — local impl
  wraps current `std::fs`/`git.rs` code paths; remote impl routes through NEW
  sidecar protocol ops (`fs_list_dir`, `fs_read_file`, `fs_write_file`,
  `fs_stat`, `git_status`, `git_diff`, `git_log`, `git_stage`, `git_commit`,
  ...). File tree, editor tabs, Git page/graph, and blame all go through the
  trait. This is the big refactor — do it incrementally: trait extraction with
  local impl first (pure refactor, tests green), then the remote impl.
- Terminal: SSH PTY channel (`ssh -t` or russh) feeding the existing vt100 grid
  instead of portable-pty.
- Browser stays LOCAL (native window). Bridge: automatic SSH `-L` port
  forwarding for declared/detected remote dev-server ports so the in-app
  browser reaches them at localhost; the `.puppy/browser.json` CDP breadcrumb
  keeps working (sidecar writes it on the remote, context note carries the
  forwarded URL).
- Risks: SSH transport library choice (russh vs spawning system ssh — spike
  first; system ssh is simpler and respects user's `~/.ssh` config, russh gives
  programmatic channels/forwarding); latency on fs ops (cache + async
  prefetch); Windows remote hosts out of scope for v1 (POSIX remotes first).

Estimated: 6-10 increments; trait refactor is the long pole.

### Phase B — "Puppy Pack" (multi-user collaboration; the official feature name)
v1 = Tiers 1+2+3, all WITHOUT forking core code-puppy:
- Tier 1 — presence + chat + activity feed: tiny hosted relay server (separate
  Rust crate, websockets, rooms keyed by project id; decision: relay from day
  one, not LAN-only) relaying user presence, a user-to-user chat panel, and
  each member's live activity feed (re-broadcast the sidecar UiEvents we
  already have: current tool, files being edited, turn status).
- Tier 2 — agent cross-awareness via context injection: same mechanism as the
  browser CDP breadcrumb — sidecar prepends a "[pack context] <user>'s puppy is
  editing X / working on Y" note to prompts when pack state exists. Zero core
  changes.
- Tier 3 — pack-coordination MCP server: a small MCP server (shipped with
  puppy-home, registered into code-puppy per-workspace) exposing agent tools:
  `claim_file` / `release_file`, `post_to_pack`, `check_teammate_status`,
  `list_claims`. Agents actively coordinate to avoid stepping on each other.
  Code-puppy already speaks MCP, so no core changes.
- Tier 4 (DEFERRED, requires core code-puppy work done together with the core
  project): real-time co-editing (CRDT), shared sessions, mid-turn
  agent-to-agent messaging via the MessageBus.
- Risks: relay hosting/auth (start with shared-secret room codes; TLS via
  reverse proxy), conflicting edits (Tier 3 claims mitigate; true merge needs
  Tier 4).

Estimated: 8-12 increments across relay + client + MCP server.

---

## Suggested sequencing
1. **Split `workspace.rs`** (P0) — land it first so future diffs are small/cohesive.
2. **Pure-fn + protocol-contract tests** during/after the split.
3. **Stand up CI** (fmt + clippy + test, cross-platform matrix).
4. **Resilience pass** (sidecar restart, output backpressure).
5. **Streaming response + dock-layout persistence + named contexts.**
6. **UX wins** (color emoji, command palette, themes) — pick by demand.
7. Feature-idea backlog only when someone asks.

**New order (2026-06-11, supersedes the above for what's left):**
1. **Phase C** — GUI managers & visual builders (agents/skills/MCP).
2. **Phase D** — right sidebar dock + dock-layout persistence.
3. **Phase A** — SSH remote sidecar (full remote IDE; `WorkspaceFs` trait
   refactor first, local-impl-only, then remote).
4. **Phase B** — Puppy Pack (relay -> context injection -> coordination MCP).

Streaming-response and the other open roadmap items (color emoji, command
palette, keyboard shortcuts, terminal polish, named contexts) get slotted in
opportunistically between phases.

---

## Known limitations
- Response is not token-streamed (thinking is); appears on completion.
- `puppy_name` is **global** Code Puppy config — one puppy across all workspaces;
  a rename shows on that workspace immediately, others on their next `ready`.
- Pruning an orphaned tool exchange drops a little context around an interrupted
  tool call (unavoidable — it's unsendable as-is).
- Live ConPTY can't be exercised in the headless test sandbox (no console host);
  it works in a real desktop session.

---

## Ground rules (don't regress these)
- The **channel / deferred-`ShellAction` invariant** is load-bearing — UI never
  touches the async agent directly.
- Crash logging in `main.rs` → `%LOCALAPPDATA%\puppy-home\crash.log` is critical
  (release uses `#![windows_subsystem = "windows"]`, no console). Keep it.
- README header is intentionally `# puppy-home`. Session persistence + kennel
  memory are confirmed working — don't break them.
- DRY / YAGNI / SOLID + the Zen of Python. New files **under 600 lines**.
- Read before you edit; prefer small `replace_in_file` diffs; `cargo build` +
  `cargo test` before every commit.

## Pointers
- Architecture deep-dive (kept current): `~/.claude/projects/D--dev-puppy-home/
  memory/puppy-home-architecture.md`.
- Original design doc: `~/.claude/plans/lets-start-a-plan-flickering-donut.{md,html}`.
- Dev run: `$env:PUPPY_HOME_CP_CMD = "D:\dev\code_puppy\.venv\Scripts\python.exe"; cargo run`
  (`PUPPY_HOME_OPEN=<folder>` auto-opens a workspace).
