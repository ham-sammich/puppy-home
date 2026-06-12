# Handoff Notes — Device Switch

Date: 2026-06-11 (Phase A remote SSH FEATURE-COMPLETE: chat+fs+editor+git)
Branch: feature/browser-plugin
State: 140 tests green, zero warnings, zero clippy lints, cargo fmt clean
Coordinator: planning-agent-8a6233 / executor: code-puppy
Agent session ids used so far: code-puppy increments under session
"phase-c-mcp-manager"; plan updates under "plan-update-2026-06-11".

This file is the source of truth on the new machine -- kennel memory does not
travel across devices.

Key commits (verified via git log; the commit carrying this updated handoff is
the new HEAD):

- d994c42 — refactor(workspace): WorkspaceGit trait + LocalGit (Phase A inc 2)
- 37bd240 — refactor(workspace): WorkspaceFs trait + LocalFs (Phase A inc 1)
- d812e7a — feat(dock): persist egui_dock split layout across restarts (Phase D)
- ed15f83 — feat(managers): paste-and-validate mode for Agents, Skills, MCP
- 3846e7e — feat(dock): open managers in a right-side dock zone (Phase D inc 1)
- e7291f5 — feat(skills): Skills Manager end-to-end (Phase C increment 2)
- b93e86d — feat(mcp): MCP Manager end-to-end (Phase C increment 1; also carried
  the reconciled .claude/claude_plan.md)

## Running on macOS (NEW 2026-06-11 — first Mac session)

This project now builds + runs on Apple Silicon. Setup that worked on
Jacobs-MacBook-Air (repo at /Users/jacob/dev/puppy-home):

- brew + uv were already installed; Rust was NOT. Installed with:
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`
  (got rustc/cargo 1.96.0, aarch64-apple-darwin). In a non-login shell you
  must `source "$HOME/.cargo/env"` before cargo is on PATH.
- Build the app only (skips the heavy wry browser plugin):
  `cargo build -p puppy-home`. Tests: `cargo test -p puppy-home`.
- Run: the backend's resolve_launch() auto-provisions via
  `uv run --with code-puppy` when uv is present, so the app JUST WORKS with no
  env var. For a deterministic version matching your own code-puppy, set
  `PUPPY_HOME_CP_CMD="$(which python3)"` from inside the code-puppy venv, then
  `./target/debug/puppy-home`. A real Aqua-session window opens (verified via
  osascript). On Mac there's no LOCALAPPDATA, so app_data_dir() falls back to
  $TMPDIR/puppy-home (sidecar.py extracted there).
- Force-kill a locked/running binary (Mac analogue of taskkill):
  `pkill -9 -f target/debug/puppy-home`.
- Browser plugin on Mac (it ships UNBUILT — separate `puppy-browser` crate):
  1. `cargo build --workspace` (or `cargo build -p puppy-browser`) -> binary at
     `target/debug/puppy-browser` (wry/WKWebView).
  2. Install where the host discovers it. plugins_dir() resolves
     `$PUPPY_PLUGINS_DIR` -> `<exe-dir>/plugins` (if it exists) ->
     `~/Library/Application Support/puppy-home/plugins`. We installed to the
     config dir: copy the binary to
     `~/Library/Application Support/puppy-home/plugins/browser/puppy-browser`
     and write a `plugin.json` next to it:
     `{"id":"browser","name":"Web Browser","version":"1.0.0","exe":"puppy-browser","min_host_version":"0.0.0"}`.
     EASIER: just run the app, open a Browser tab, click "Install from local
     build" (it copies target/debug/puppy-browser + writes the manifest for you
     via install_from_local_build()).
  3. CAVEAT: window embedding (reparenting the webview into the Browser tab) is
     WINDOWS-ONLY. On macOS puppy-browser opens as a SEPARATE floating window;
     toolbar controls (navigate/back/forward/reload/devtools) still drive it
     over stdin. Embedding the WKWebView into the egui viewport is unimplemented
     (report_handle returns 0 on non-Windows). Verified the binary launches a
     real Aqua window on this Mac.
- Verification standing order on Mac: drive sidecar.py headlessly over stdio
  (spawn `python3 sidecar/sidecar.py`, write JSON ops, read JSON events) +
  `cargo test`. GUI screenshots remain unreliable.

## Where we are

Executing the "New feature roadmap (2026-06-11)" in .claude/claude_plan.md,
priority order C -> D -> A -> B. Increments run through planning-agent session
id `phase-c-mcp-manager` (code-puppy does the hands-on work).

- Increment 1 — MCP Manager — DONE (b93e86d): sidecar ops list_mcp_servers /
  set_mcp_enabled / add_mcp_server, protocol contract tests,
  src/views/mcp_manager.rs dockable tab + add-server wizard, top-bar MCP button.
- Increment 2 — Skills Manager — DONE (e7291f5): sidecar ops list_skills /
  get_skill / set_skill_enabled / save_skill, src/views/skills_manager.rs +
  skills_wizard.rs + views/common.rs (shared helpers), top-bar Skills button.
- Increment 3 — Agent Manager + visual builder — DONE (uncommitted as of this
  edit; see below): sidecar ops list_agent_configs / get_agent_config /
  save_agent_config / delete_agent_config / clone_agent_config (wrapping
  code-puppy's json_agent + agent_manager APIs). New src/views/agent_manager.rs
  (panel) + agent_wizard.rs (4-step visual builder: Basics, Prompt, Tools,
  Review). Top-bar "Agents" button -> dockable Tab::AgentManager. Built-in
  Python agents shown read-only with a Clone-to-editable-JSON button; JSON
  agents are editable + deletable (with an inline delete confirmation; can't
  delete the active agent). Builder covers name/display_name/description/model/
  system_prompt/user_prompt/tool-multiselect/MCP-bindings; the Review step
  renders the exact on-disk JSON (compose_preview mirrors the sidecar's
  json.dumps insertion order + optional-field omission, unit-tested). All 5
  ops verified headlessly against real code-puppy (list/get/save/delete +
  clone roundtrip). 115 tests green, zero warnings, zero clippy, fmt clean.
- Test suite: 115 tests green, zero warnings, zero clippy lints, fmt clean.

## Phase D — DONE (2026-06-11)

- Inc 1 (3846e7e): right-side dock zone. Managers (MCP/Skills/Agent) split off a
  ~28% right sidebar and cluster there as tabs via egui_dock (drag in/out,
  resize). `app::open_panel_tab` + `is_panel_tab` own this; first panel carves
  the sidebar, later panels reuse it.
- Inc 2 (d812e7a): dock-layout persistence. `session.json` now stores the full
  egui_dock tree (`Session.layout: Option<DockState<SavedTab>>`), not just the
  open set. New `src/dock_layout.rs` is the glue:
  - `SavedTab` mirrors `shell::Tab` with device-independent keys (workspace
    PATHS, not runtime ids). Browser tabs are NOT persisted.
  - Save: `DockState<Tab>` -> `DockState<SavedTab>` via egui_dock
    `filter_map_tabs` (drops browsers/closed chats, collapses empty nodes),
    then `session::normalize_layout_rects` zeroes node rects (fresh leaves carry
    `Rect::NOTHING`/inf, which JSON writes as `null` and refuses to read back;
    egui_dock recomputes rects each frame so zeroing is lossless).
  - Restore: `DockState<SavedTab>` -> `DockState<Tab>`, remapping each chat's
    path to its freshly spawned WorkspaceId via a path->id map built while
    reopening folders; unmappable tabs drop. `ensure_core_tabs` backstops a
    stale layout (guarantees Dashboard + a chat per reopened workspace).
  - Writes gated on a rect-free STRUCTURAL signature (arrangement + active tab +
    split fractions) so resizing the OS window doesn't churn the file. Verified
    headlessly: layout round-trips on relaunch (path remaps, no crash), zeroed
    rects deserialize, no idle file churn.
  - Enabled egui_dock's `serde` feature; `Tab` gained `Debug`.

## Also done this session — paste mode for the managers (ed15f83)

Each create/edit wizard (Agents, Skills, MCP) gained a `Form | Paste` toggle:
paste a whole config and **Format** (syntax-check + tidy) or **Save** (validate
+ write), errors shown inline. All funnel through the existing save ops. Shared
`EditMode`/`mode_toggle`/`paste_editor` in `views/common`. To stay under 600
lines this split two wizards into directory modules with a `steps` child
(`views/agent_wizard/{mod,steps}.rs`, `views/mcp_wizard/{mod,steps}.rs`) and
extracted the MCP wizard out of `mcp_manager.rs` (651 -> 222 lines). Paste
formats: agents = full agent JSON (string or {name,..} mcp_servers); skills =
full SKILL.md (frontmatter + body); mcp = a `{"name": {..}}` entry (unwraps an
outer `mcpServers` wrapper, infers transport from a `type` field or command/url).

## Phase A — Remote SSH — IN PROGRESS

The pure-refactor half (extract traits with local impls, no behaviour change)
is DONE:
- Inc 1 (37bd240): `WorkspaceFs` trait + `LocalFs` in src/workspace/fs.rs.
  Workspace holds `Arc<dyn WorkspaceFs>`; the editor (open/save/delete/rename/
  new), the file tree (`render_dir` takes `&dyn WorkspaceFs`), and the
  AI-wrote-a-file refresh route through it. Host config/state stays on std::fs.
- Inc 2 (d994c42): `WorkspaceGit` trait + `LocalGit { root }` in src/git.rs
  (wraps the existing free fns). Workspace holds `Arc<dyn WorkspaceGit>`; ~30
  call sites in git_view.rs/git_graph_view.rs/mod.rs call `self.git.<op>()`.
  The async status worker clones the Arc (trait is Send+Sync).

- Cleanup (f570924): split editor.rs + mod.rs back under the 600-line budget.
  New tree_ops.rs (file-tree create/rename/delete + modals) and events.rs
  (UiEvent/BackendMessage folding: apply_event/on_message/set_status/etc).
- Inc 3 (f4a5207): system-`ssh` transport spike. New src/backend/ssh.rs --
  SshTarget{host,user,port,identity} + parse([user@]host[:port]), base_ssh()
  (flags: -T, BatchMode=yes, ConnectTimeout=10, StrictHostKeyChecking=accept-new),
  provision_command(), launch_command(cwd,launcher), sh_quote(); 9 offline unit
  tests. CodePuppy::spawn_remote() (backend/mod.rs) provisions sidecar.py over
  ssh ('mkdir -p ... && cat > ...', bytes on stdin) then launches it; stdio
  carries the same JSON protocol. #[allow(dead_code)] until inc 4 wires it.
  NOT live-validated (ssh localhost refused on this Mac -- Remote Login off).
  Override remote launcher via PUPPY_HOME_REMOTE_CP_CMD.

- Inc 4 (efb8df1): connect-to-remote dialog. ssh.rs config_hosts() reads Host
  aliases from ~/.ssh/config (follows Include, single-level glob, skips
  wildcards). views/remote_connect.rs = the dialog (host list + free-text
  [user@]host[:port] + remote path + spinner). Connection runs off-thread
  (PuppyApp::begin_remote_connect/poll_remote in app/remote.rs); Supervisor::
  adopt() factored out. Workspace.remote_label drives an honest tree-panel
  placeholder (chat works; files/git are inc 5). Split app.rs -> app/mod.rs
  (587) + app/remote.rs (97) to stay under budget.

- Inc 5a (3eb205c): READ-ONLY remote tree + editor over the sidecar stdio
  channel (no extra ssh round-trips). sidecar.py: fs_list_dir/fs_read_file ->
  fs_result (validated live by piping ops to a local sidecar). backend/remote.rs:
  RemoteState (cache + in-flight table + shared stdin) + RemoteFs (impl
  WorkspaceFs): read_dir cached+async-filled (never blocks the per-frame tree),
  read_to_string blocks on a one-shot w/ 20s timeout. KEY: the stdout reader
  routes fs_result OFF the UI thread (a blocking read on the UI thread can't get
  its reply via UiEvent/apply_event without deadlock). CodePuppy.stdin is now
  Arc<Mutex<>>; spawn_remote returns the RemoteFs; Workspace::new takes an
  injectable fs; Supervisor::adopt threads it. Mutations return 'not supported
  yet'; tree hides +File/+Folder and shows 'user@host - read-only'.

- Inc 5b (c61f54c): remote EDITING -- fs_stat/write/mkdir/create/remove/rename
  ops; RemoteFs implements every WorkspaceFs method. Generalised RPC core:
  call()/call_unit() block on a one-shot reply Value (20s); listings+stats
  async-cached; mutations invalidate parent listings. Validated live
  (mkdir->write->read->rename->list->remove round trip).
- Inc 5c (8c6f292): git over SSH -- RemoteGit. git.rs refactored to a GitRunner
  abstraction so PARSING is shared; LocalRunner shells out, RemoteRunner runs
  'git_run' over the RPC channel; the 24-method WorkspaceGit impl is one
  impl_workspace_git! macro reused by both. sidecar git_run validated live
  (is-inside-work-tree, status --porcelain). Workspace::new now takes injectable
  fs+git; Supervisor::adopt threads both; reader demuxes fs_result+git_result.

>>> Phase A (Remote SSH) is FEATURE-COMPLETE: a remote workspace's chat, file
    tree, editor (read+write), and full git all work on an SSH host, all over
    the sidecar's single stdio channel. <<<

Post-Phase-A polish landed:
- Git HTTPS credentials modal (push/pull/fetch auth) -- detect auth failure,
  collect user/token, retry via a one-shot credential helper (env-fed, nothing
  stored). Works local + remote.
- Chat "File" button (next to Image): browses the workspace via WorkspaceFs
  (local OR remote), inserts an @relpath reference into the composer. Shared
  browser UI in views/path_browser.rs.
- Remote-connect folder browser: "Browse the remote host..." lists dirs over SSH
  (ls -1Ap, off-thread, starts at login home) so you can pick the working dir
  visually instead of typing it. Reuses path_browser. ssh::parse_listing +
  list_dir_command are unit-tested.
- File-budget hygiene: extracted workspace/file_picker.rs, pending_prompt.rs,
  git_creds.rs, git/runner.rs; composer.rs back to 594.

PHASE B (Puppy Pack) in progress:
- Inc B1: relay/ workspace crate ('puppy-relay [port]', default 9220) -- rooms
  keyed by code (code = shared secret), presence/chat/activity re-broadcast,
  relay stamps from+ts. TRANSPORT: line-JSON over TCP (deviation from the
  plan's websockets -- zero deps, same wire pattern as the sidecar; protocol is
  transport-agnostic, ws can be layered later). 7 unit + 3 real-socket e2e
  tests + live binary run with two Python clients.
- Inc B2: in-app client + panel. src/pack.rs (PackClient: reader thread ->
  PackEvent channel + repaint; chat/activity/leave; reuses puppy-relay's
  protocol types via path dep -- ONE wire definition). views/pack_panel.rs
  (join form -> room view: members + latest activity, bounded feed, chat).
  Tab::Pack is a panel tab (right dock zone), persisted via SavedTab::Pack.
  App broadcasts a throttled 'status' activity summary of all workspaces
  (only when changed). Integration test: PackClient vs in-process relay.
- Inc B3 (Tier 2) DONE: puppy names + pack context injection. Protocol v2
  (PROTO_VERSION=2): Join/Joined/MemberJoined carry `puppy` (MemberInfo struct);
  member list shows 'user [dog] Puppy'. App drops .puppy/pack.json (members +
  puppies + activity + last-10 chat + `updated` stamp) into every LOCAL
  workspace -- written on change + re-stamped every 5 min, removed on leave;
  sidecar's module-level pack_context() injects '[pack context] ...teammate
  activity + recent chat + coordinate-don't-collide guidance' into every prompt
  (staleness-gated, 15 min). Validated by importing sidecar and exercising
  pack_context against a real breadcrumb (incl. self-exclusion + stale-reject).
  App-side glue extracted to src/app/pack_sync.rs (app/mod.rs back to 620).
  NOTE: remote workspaces don't get the breadcrumb yet (write via RemoteFs is
  possible but chatty; revisit).
- Inc B4 (Tier 3) DONE: agent coordination. Protocol v3: ClaimInfo + roomless
  one-shot ops (claim/release/list_claims/post) usable as a connection's first
  message; relay keeps claims per-room (1h TTL, die with the room, holder-only
  release, same-user re-claim refreshes) and broadcasts 'claims' on change.
  AGENT SIDE: sidecar/pack_helper.py (dependency-free CLI: claim/release/
  claims/post/status) is embedded via include_str! and dropped at
  .puppy/pack_helper.py next to pack.json (which now carries relay/helper/
  claims); pack_context() teaches the agent to claim BEFORE editing + post
  plans. Panel shows a CLAIMS section. DEVIATION from plan's 'MCP server':
  helper-CLI via the agent's shell tool (the cdp_helper.py pattern) -- zero
  config mutation; wrap in MCP later if demand appears.
  VALIDATED LIVE: real relay + joined member + helper: claim -> broadcast,
  rival refused w/ holder named, post lands as \"Rufus (jacob's puppy)\",
  release empties. 155 tests total.
>>> PHASE B (Puppy Pack v1, Tiers 1-3) is FEATURE-COMPLETE. <<<
- NOTE: protocol v3 -- BOTH devices must rebuild (mismatches get a clean
  'protocol mismatch' relay error).
PERF PASS (Windows sluggishness, 2026-06-11): found + fixed four per-frame
costs that hit Windows hardest (NTFS/Defender/expensive process spawns):
1. Transcript rendered EVERY entry every frame (commonmark re-parse + syntect;
   ScrollArea doesn't cull) -> now renders the last 120 with a 'Show older'
   opt-in (workspace.transcript_show_all).
2. poll_git spawned 'git status' every 2s/tab + a 1.5s repaint floor -> 4s
   cadence, skipped entirely while the window is unfocused, floor matches.
3. File tree enumerated every expanded dir from disk every frame -> CachedFs
   (fs.rs): 2s-TTL read_dir cache over LocalFs; mutations through it
   invalidate instantly (unit-tested). Remote keeps its own event cache.
4. persist_session built the dock signature every frame -> throttled to 1s +
   unconditional final write in on_exit.
Still-known perf debt: editor syntect re-highlights visible files per frame
(big files); browser overlay does Win32 calls per frame when a browser tab is
open; egui_commonmark has no layout cache. Windows ops advice: run RELEASE
builds (debug egui is dramatically slower) + consider Defender exclusions for
the repo folder + git.exe; the perf HUD (top bar) shows frame cost/repaints.

- Pack polish backlog: remote-workspace breadcrumbs; GUI claim buttons;
  claims surfaced on the Dashboard; relay auth beyond room codes (TLS/proxy =
  the ws upgrade path).
- To try it: cargo run -p puppy-relay (or ./target/debug/puppy-relay), then
  top bar -> Pack -> join the same room code from two machines/instances.

Remaining / next:
- LIVE-VALIDATE the full Rust<->SSH round trip against a real reachable host
  (ssh localhost is off here -- Remote Login disabled). EVERY sidecar op (fs +
  git) is validated locally by piping JSON to a local sidecar; the Rust RPC
  layer compiles + is logically sound but hasn't driven a real remote sidecar.
  First thing to do once a host exists: open the Connect dialog, point it at the
  host + a repo path, confirm tree/editor/git light up.
- Polish ideas: cache TTL/manual refresh for the remote tree; ControlMaster is
  NOT used (we ride the sidecar pipe, so it's unnecessary); surface remote op
  errors more visibly; consider a remote-git status poll interval.
- After Phase A: Phase B (Puppy Pack).
- Pre-existing over-budget files (NOT from this session; split later):
  src/workspace/view.rs (730, was 719 at session start) and
  src/backend/mod.rs (~1450). Everything created this session is < 600.
- Out-of-scope-so-far local fs that a remote workspace will also need: the
  `.puppy/browser.json` + `.gitignore` breadcrumb writes in view.rs (still
  std::fs) and git.rs `untracked_content` reads the file via std::fs inside the
  free fn (LocalGit path) — fine for local, revisit for remote.
- SSH TRANSPORT DECISION: RESOLVED -> system `ssh` (see inc 3 above). Revisit
  `russh` only if we later need programmatic port-forwarding the binary can't do.
- Then: connection profiles + "Open Remote Folder..." flow; new sidecar
  protocol ops (fs_list_dir/read/write/stat, git_status/diff/log/...) +
  RemoteFs/RemoteGit impls; async + caching for fs ops (latency); terminal over
  SSH PTY; port-forwarding for the in-app browser. See claude_plan.md Phase A.

After Phase A: Phase B (Puppy Pack). Full details/risks in .claude/claude_plan.md.

## Open items / tech debt (from last increment reports)

- Agent Manager: MCP bindings are saved as a plain name list (shorthand ->
  auto_start=true). code-puppy also supports the dict form with per-server
  auto_start; a future toggle could expose it. Cloning a JSON agent that lives
  in a project dir still clones into the USER agents dir (code-puppy's
  clone_agent behaviour) — fine for now.
- RESOLVED (ed15f83): agent_wizard.rs split into a directory module; mcp_manager
  wizard extracted to views/mcp_wizard/ (mcp_manager now 222 lines).
- FORM-mode save/delete errors (Agent/Skills/MCP `add_mcp_server` etc.) still
  surface in the chat transcript, not the manager panel. PASTE-mode errors DO
  show inline in the wizard. Routing the form/op-result errors to the panels is
  still open — good next small task (Jacob flagged interest).
- Browser tabs are intentionally not restored by dock-layout persistence (the
  plugin doesn't restore tabs across runs); a closed-then-saved layout simply
  omits them. session.json now also carries egui_dock's i18n "translations"
  block (harmless bloat from serializing DockState).
- Editing a plugin-sourced skill re-saves it as a user copy
  (~/.code_puppy/skills) rather than editing in place. Acceptable for now;
  consider an explicit "fork to user" affordance later.
- Skill directories without a SKILL.md are silently skipped by discovery; no
  warning surfaces anywhere.

## How to resume

Dev setup on a new machine: clone the repo; you need a code_puppy venv on the
box. Then (PowerShell):

    $env:PUPPY_HOME_CP_CMD = "<path>\code_puppy\.venv\Scripts\python.exe"
    cargo run

Release packaging: scripts/build-release.ps1.

Rebuild/relaunch standing order: if target\debug\puppy-home.exe is locked by a
running instance, force-close it yourself — do not ask:

    taskkill /f /im puppy-home.exe
    cargo build
    # then launch target\debug\puppy-home.exe in the background

Ground rules:

- No emoji anywhere in the GUI codebase (emoji-guard test enforces this).
- Files stay under 600 lines; split components when they grow past it.
- Zero warnings, zero clippy lints; full test suite green before commit.

Where the institutional memory lives: architectural decisions and per-increment
notes are saved in the puppy kennel, repo wing (repo:D:\dev\puppy-home). Recall
with the kennel tools before starting Increment 3 — the Skills Manager entry
documents the sidecar APIs and the compose_preview byte-identity contract.
