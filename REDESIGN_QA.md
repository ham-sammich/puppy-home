# Redesign QA — `redesign/gpui` (final scope, Phase G)

A ~20-minute manual walkthrough of the full GPUI app before mainline.
Prereqs: two folders you can open as workspaces (one a git repo, ideally
with a `plans.md`), network access for §8, and nothing else — the Den
section now hosts its own relay.

Pass criterion for every step: the described behavior, no visual
glitches, and no fan-spinning idle CPU (GPUI is retained-mode; idle
discipline shows up as flat CPU in Activity Monitor — including with a
Browser tab open while the app is unfocused, per the G1 audit fix).

Probe shortcuts (optional for humans): `PUPPY_GPUI_OPEN=/path` opens a
workspace, `PUPPY_GPUI_PROBE=1` logs state lines,
`PUPPY_GPUI_PROMPT="..."` fires one prompt, `PUPPY_GPUI_SCREEN=chat`
jumps to chat, `PUPPY_GPUI_DEN=addr,room,user` joins a relay,
`PUPPY_GPUI_DEN=host` exercises self-hosting.

## 1. Dashboard & toolbar (3 min)

1. Launch with no workspaces. Empty hint shows; CPU near zero at idle.
2. Toolbar (wraps to two rows on narrow windows): brand, identity chip
   (`{avatar} {puppy}`), ＋New Chat, Open Folder…, Connect Remote…, Web,
   fleet stats, version chip (`v0.0.xxx`), MCP / Skills / Agents /
   Models / Config, Join Den, Motion, Theme.
3. Open Folder → card fades in (entrance animation). Send a prompt from
   its chat, return: live card — pulsing dot, glowing avatar ring
   (pulse, not spin), pack vocabulary + tool verb, ticking clock, tok/s
   sparkline, last-prompt inset (hover for full text), and the 3px
   context bar once the sidecar reports ctx-% (no bar before — a 0% bar
   would be a lie). Cost cell shows `~$…` (estimated marker) or an em
   dash, never $0.00.
4. Card actions: Pause → "Napping" + toast; Resume; Steer (inline input,
   now/queue); Stop. Kill the sidecar externally → "Stuck" + Restart
   revives.
5. The Whistle button (next to the H1) spawns a home-dir instance instantly; the
   toolbar ＋New Chat does the same but lands you in its chat, focused.
6. Grid → List → Focus; restart: view remembered. Motion: off freezes
   every pulse/bob/blink (static rings stay legible); restart: setting
   survives (shared `reduce_motion`).

## 2. Chat: composer, transcript, sessions, logs (4 min)

7. Empty chat: breathing puppy (your avatar choice, see §9) + z z z.
8. Status line: state dot + "Ready · agent · model"; once a turn has
   run, a `ctx N%` chip appears when known (weak <60, amber <85, red
   above); right side: `/cmds ▾` pill — popover lists /pop, /pop N…,
   /compact, /truncate N…, /dump_context, /clear. Direct ones send
   exactly like typing (mid-turn: no-op, same as typing); the `N…` ones
   seed the input for you to complete. Typed slash commands still work
   identically.
9. Composer styles via the footer gear: Classic / Unified / Palette /
   Guided; the draft survives switches; restart remembers the style.
10. Input: full cursor/selection/IME, soft wrap, up/down across lines,
    prompt-history recall (Up at top), Enter sends, Shift+Enter
    newlines (grows to 8 rows), Enter mid-turn steers (now/queue
    toggle). `/` and `@` completion palettes; @File button inserts
    chips; paste an image → thumbnail chip rides the next prompt.
11. Transcript: you-turns (your avatar) and puppy-turns (markdown:
    headings, bullets, fences, tables, links); tool chips; diff chips
    expanding to green/red rows; thinking folds auto-collapse at
    turn-end; >120 entries → "Show older".
12. Sessions button → browser overlay: filter, preview (last
    exchanges), Load (transcript + autosave swap), New chat (rotates
    autosave). Logs button → tail panel (last 200 lines, mono).
13. Explorer tree: lazy dirs, A/M/D markers while the agent edits,
    right-click panel (new file/folder, rename, delete), Changes list
    with +adds/−dels.

## 3. Editor (2 min)

14. Click a file in the tree → editor tab: syntect highlighting, soft
    wrap, full editing (≤200 KB highlighted; larger files plain).
    Edit → dirty dot; Cmd+S saves; close via the tab's x. The Changes tab
    shows the working-tree diff with stage/unstage per file.

## 4. Git view & graph (2 min)

15. Git toolbar button (git workspaces): status list (staged/unstaged/
    untracked w/ checkboxes), commit box (message + Commit button),
    branch chip. The graph renders commit lanes with refs; clicking a
    commit shows its files; push/pull buttons surface errors inline
    (creds prompts ride the credential-helper flow).

## 5. Terminal (1 min)

16. Terminal toggle (chat toolbar or Classic composer row): fills the
    chat area; run `ls`, `top` (colors, live update), arrow keys +
    Tab reach the shell (bindingless key context); scrollback wheel +
    banner; resize the window → PTY follows next frame. Toggle off →
    transcript returns. (Feature parity with egui is exact: fg/bg/
    inverse/underline; no selection-copy, no mouse reporting.)

## 6. Managers: MCP / Skills / Agents / Models / Config (3 min)

17. Each manager opens a centered overlay over any screen, served by
    the first ready sidecar ("via {workspace}" subtitle), filter field,
    Refresh, 2-5-10s polling cadences — except Config, which is
    file-based and works sidecar-less.
18. MCP: list w/ enabled toggles + status; Add → wizard (stdio/SSE/
    HTTP steps or JSON paste w/ syntect); edit/remove round-trip.
19. Skills: user+project lists, toggles, detail view; Create/Edit →
    wizard (form steps or SKILL.md paste mode).
20. Agents: list + detail; Clone/Delete (confirm); ＋Create agent →
    JSON wizard (tools/MCP checkboxes, prompts, paste mode w/ JSON
    highlighting); Create with Agent Creator → fresh $HOME session
    already running code_puppy's agent-creator — describe the agent
    conversationally.
21. Models: catalog rows (provider type, ctx length, "custom" badge,
    ● active); Set active switches the serving workspace's model;
    "Edit extra_models.json" → JSON editor, Save validates + writes
    ("restart workspaces to load"); the x button removes custom entries.
22. Config: puppy.cfg as a settings list (priority keys pinned,
    banner_colors alphabetical); Edit inline → Save rewrites ONE line
    (comments/sections survive); secret-looking keys masked + locked.

## 7. Version & updates (1 min)

23. Toolbar `v0.0.xxx` chip (live from the sidecar) → About panel:
    Check for updates hits PyPI (offline → inline error); when newer,
    Update now refreshes uv's cache and reports the landed version +
    "restart workspaces to apply". Update is bounded (5-min kill).

## 8. Den: join, HOST, collaborate (3 min)

24. Join Den with no relay running → "Host a Den": relay spawns
    locally (binary next to the app, cargo fallback in dev), you
    auto-join, header shows `HOSTING · share ip:port · room` + Stop
    hosting. Join from a second instance using that line.
25. Roster cards (state, agent·model, tok/s sparkline, verb) within
    ~3s; Nudge; feed (owner colors, → addressing, review badges,
    bottom-pinned, latest 150 + show all); board (add/retitle/assign/
    move/delete, live on both sides); plans.md share → plan card.
26. Presence: unfocus → idle on the other side; refocus + click flips
    back. Stop hosting → both sides disconnect (rooms are in-memory).
    Quit the host app with the relay up → the relay self-exits (PID
    watchdog; verify no `puppy-relay` survives in `ps`).

## 9. Avatars & identity (1 min)

27. Click the identity chip → Avatars panel: switch You/Puppy, pick
    from the grid or type any emoji + Use; transcript, empty state,
    ask headers, dashboard lede, and the chip update instantly;
    restart: persisted (session.json, shared with the egui shell —
    egui renders your choice on its next launch).

## 10. Browser, remote, themes (2 min)

28. Web button → embedded browser tab (address bar, back/fwd, popout
    to floating, re-embed; close kills the child process). Switch
    to Dashboard → embed hides; minimize → hides; CPU flat while
    unfocused with the tab open (G1 fix).
29. Connect Remote… → ssh target + folder browse; a remote workspace
    behaves like a local one (badge in card meta). On a host without
    code_puppy: ssh-fallback mode is flagged in the session row; creds
    push (toolbar key icon) arms → confirms → pushes ~/.code_puppy
    credentials (never logged).
30. Theme button cycles Dark/Light (+ custom theme files if present);
    restart: theme survives. All overlays/popovers/toasts re-skin.

---

## Branch stats (refreshed at final scope, Phase G1)

Recorded on macOS (Apple Silicon), 2026-06-13, commit `ba196a8`:

| Metric | Value |
|---|---|
| `cargo build --release` (clean, wall time) | 1m 53s (real 113.1s · user 561.9s) — faster than 2.5's 2m 27s (same deps, better parallelism) |
| Release binary size (`target/release/puppy-home`) | 12 MB (was 6.8 MB at 2.5 — syntect grammars/themes + editor + git graph + terminal + managers since) |
| Branch diff vs fork point (`git diff --stat 0f00eed`) | 89 files, +29,675 / −393 |
| Commits since fork | 69 |
| Dependency count (Cargo.lock packages) | 970 (unchanged since the 2.5 audit — every phase since rode existing deps) |
| Test count | 223 green (205 app: 203 unit + pty_live + vt100_grid; 18 relay) |
| Compiler warnings | 0 (one pre-existing `block v0.1.6` future-incompat from `objc`, same as base) |

### Known deliberate deviations (final)

- Avatar ring **pulses** instead of spinning (no cheap rotation
  transform at this gpui pin).
- Grid view flex-wrap sizing; last-row cards may stretch wider than a
  CSS grid would.
- Markdown is an in-house subset — now with tables + links; still no
  images (Zed's markdown crate rejected for dependency weight).
- Terminal: fg/bg/inverse/underline only (matches egui exactly); no
  selection-copy or mouse reporting on either branch. vt100 0.16 has
  no damage API — full-grid reshape per render, bounded, only while
  the terminal is visible.
- Manager wizards: env/header pair rows are KEY=VALUE lines (one
  field), not add/remove row pairs.
- Den: plan cards cap at 8 checklist rows; kanban drag-drop is
  menu-driven (same as egui); avatars don't ride the roster (no
  protocol slot — ledgered); one den at a time (single PackClient —
  multi-den ledgered).
- egui shell has no avatar picker (renders the GPUI choice on next
  launch); cost cell is `~$` estimated when the sidecar lacks pricing.
- Composer input: no cursor blink. Soft wrap, vertical cursor moves,
  and history recall all landed in Phase B (the old deviations list is
  obsolete).
- Emoji render in **full color** (a GPUI capability the egui branch
  lacks).
