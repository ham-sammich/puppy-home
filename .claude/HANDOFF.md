# Handoff Notes — Device Switch

Date: 2026-06-11
Branch: feature/browser-plugin
State: 106 tests green, zero warnings, zero clippy lints, cargo fmt clean
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
- Test suite: 106 tests green, zero warnings, zero clippy lints, fmt clean.

## Next up

Increment 3 — Agent Manager + visual guided agent builder:

- New sidecar ops: list_agent_configs / get_agent_config / save_agent_config /
  delete_agent_config, wrapping code-puppy's JSON agent configs
  (sidecar/sidecar.py + src/backend/protocol.rs wires).
- New panel: src/views/agent_manager.rs + builder wizard covering name / model /
  system prompt / tool selection, following the same dockable Tab + top-bar
  button pattern as mcp_manager.rs / skills_manager.rs. Reuse the shared
  helpers in src/views/common.rs. Keep the wizard in its own file from day one
  (see skills_wizard.rs precedent).

After Phase C: Phase D (right sidebar dock + dock-layout persistence with
workspace-id remapping), then Phase A (SSH remote sidecar), then Phase B
(Puppy Pack). Full details and risks live in .claude/claude_plan.md.

## Open items / tech debt (from last increment reports)

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
