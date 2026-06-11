# Handoff Notes — Device Switch

Date: 2026-06-11 (updated after Increment 3 + macOS bring-up)
Branch: feature/browser-plugin
State: 115 tests green, zero warnings, zero clippy lints, cargo fmt clean
Coordinator: planning-agent-8a6233 / executor: code-puppy
Agent session ids used so far: code-puppy increments under session
"phase-c-mcp-manager"; plan updates under "plan-update-2026-06-11".

This file is the source of truth on the new machine -- kennel memory does not
travel across devices.

Key commits (verified via git log; the commit carrying this updated handoff is
the new HEAD):

- 71c8c06 — docs: handoff notes + reconciled plan for device switch
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

## Next up

Phase C's three managers (MCP, Skills, Agent) are all DONE. Next is **Phase D**:

- Right-side dock zone the managers (MCP/Skills/Agent), perf HUD, and future
  Puppy Pack chat can be dragged into/out of via egui_dock.
- Land dock-layout persistence: persist the egui_dock split layout in
  session.json (not just the open set). Needs runtime workspace-id remapping on
  restore (the WorkspaceId in saved Tabs won't match freshly-spawned ones).

After Phase D: Phase A (SSH remote sidecar; WorkspaceFs trait refactor first),
then Phase B (Puppy Pack). Full details and risks live in
.claude/claude_plan.md.

## Open items / tech debt (from last increment reports)

- Agent Manager (inc. 3): MCP bindings are saved as a plain name list
  (shorthand -> auto_start=true). code-puppy also supports the dict form with
  per-server auto_start; a future toggle could expose it. Also: save/delete
  errors surface in the chat transcript, not the Agent panel (same gap as
  add_mcp_server, below). Cloning a JSON agent that lives in a project dir
  still clones into the USER agents dir (code-puppy's clone_agent behaviour) —
  fine for now.
- agent_wizard.rs is 587 lines (close to the 600 budget). If it grows, split
  the per-step renderers (step_basics/step_prompt/step_tools/step_review) into
  an agent_wizard_steps.rs.


- src/views/mcp_manager.rs is 651 lines (over the 600-line budget). Extract its
  add-server wizard into mcp_wizard.rs (mirror skills_wizard.rs) on next touch.
- add_mcp_server errors surface in the chat transcript, not the MCP panel.
  Route them to the panel for proper inline feedback.
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
