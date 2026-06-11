# Handoff Notes — Device Switch

Date: 2026-06-11 (updated after Phase A inc 3: system-ssh transport spike)
Branch: feature/browser-plugin
State: 138 tests green, zero warnings, zero clippy lints, cargo fmt clean
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

Still TODO for Phase A (the real remote work -- needs decisions):
- Inc 4 NEXT: connection profiles + "Open Remote Folder..." flow that builds an
  SshTarget and calls spawn_remote. Then wire RemoteFs/RemoteGit (the traits
  from inc 1+2) to new sidecar protocol ops (fs_list_dir/read/write/stat,
  git_status/diff/log/...) so the tree/editor/git work against the remote.
- Live-validate spawn_remote against a real host (or enable Remote Login for
  `ssh localhost`). backend/mod.rs is ~1370 lines (pre-existing, over budget)
  -- candidate for its own split later.
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
