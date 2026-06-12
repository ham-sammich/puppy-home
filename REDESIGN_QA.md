# Redesign QA — `redesign/gpui` (Phase 2)

A ~10-minute manual walkthrough of the Command Center redesign on the GPUI
branch. Prereqs: one or two folders you can open as workspaces (at least one
being a git repo with a `plans.md` is ideal), and optionally a second
terminal running `puppy-relay` for the Den section (a local
`cargo run -p puppy-relay` works).

Pass criterion for every step: the described behavior, no visual glitches,
and no fan-spinning idle CPU. (No perf HUD on this branch — GPUI is
retained-mode; idle discipline shows up as flat CPU in Activity Monitor.)

Probe shortcuts used during development (optional for humans):
`PUPPY_GPUI_OPEN=/path` auto-opens a workspace, `PUPPY_GPUI_PROBE=1` logs a
state line, `PUPPY_GPUI_PROMPT="..."` fires one prompt,
`PUPPY_GPUI_SCREEN=chat` jumps to the chat, `PUPPY_GPUI_DEN=addr,room,user`
joins a relay room.

## 1. Dashboard

1. Launch the app with no workspaces open. Dashboard shows the empty hint
   ("No agents running..."). CPU should sit near zero while you don't move
   the mouse (drain loop idles at 1s; no animations exist while nothing is
   live).
2. Open a folder (toolbar "Open Folder..."). A card fades in (entrance
   animation): dir-name title, `agent · ~path` mono meta line, model pill,
   "Resting" state.
3. Send a prompt from the workspace chat (Open ->), return via the
   Dashboard tab. The card goes live: pulsing state dot, glowing avatar
   ring (pulse, not spin — GPUI deviation, see below), pack vocabulary
   (Fetching / Sniffing / Digging) + current tool, elapsed clock ticking,
   tok/s + mini sparkline filling in, last-prompt inset shows your prompt
   (hover it for the full text tooltip).
4. Header: lede counts update ("1 on the hunt"); Throughput tile shows the
   aggregate sparkline. Spend tile shows an em dash (never $0.00).
5. Card actions while live: Pause -> "Napping" (amber) + toast; Resume ->
   live again; Steer -> inline input expands (type, toggle "queue", Nudge)
   -> last-prompt tag shows "+1 queued"; another with "now". Stop ->
   turn cancels, card returns to Resting.
6. Model pill -> popover lists the sidecar's models; pick one -> toast, and
   the pill + chat status line update after the sidecar re-announces.
7. Make the agent ask you something (e.g. prompt "use ask_user_question
   before doing anything"). Card turns pink "Needs you" + the attention
   banner appears with the question and an "Answer {dir} ->" button that
   jumps to the chat, where the answer panel sits above the composer:
   click options (radio/checkbox), try "Other..." free text, Submit.
   The turn resumes.
8. Kill the sidecar process externally. The card shows "Stuck" red with a
   Restart action; Restart revives the session.
9. Segmented control: Grid -> List (dense table with quick actions) ->
   Focus (single centered column, max 880px). Restart the app: the chosen
   view is remembered (session.json — same field the egui branch writes).
10. Toolbar "Motion: on" -> off (reduce motion). Dot pulses, avatar rings,
    entrance fades, the empty-state bob, and the Den LIVE blink all freeze
    (static rings remain so state stays legible). Restart — the setting
    survives (shared `reduce_motion`).

## 2. Workspace Chat

11. Open a workspace chat with no conversation. The empty state shows:
    breathing puppy, floating z z z, "How can {puppy} help you?". Under
    reduce-motion both freeze.
12. Composer dock: status line reads "Ready · {agent} · {model}" with a
    state-colored dot.
13. Footer gear ("Composer: Classic") -> popover lists Classic / Unified /
    Palette / Guided with descriptions. Switch through all four; the input
    draft survives style switches (one shared ChatInput entity). Restart —
    the style is remembered (`composer_style`, shared field).
14. Input behaviors (any style): type with full cursor/selection (mouse
    drag, shift-arrows, cmd-A), cmd-V paste, IME composition (e.g. pinyin —
    marked text underlines), Enter sends, Shift+Enter newlines (the input
    grows, up to 8 rows). While a turn runs, Enter steers ("now" delivery)
    instead.
15. Typing `/` shows the completion palette above the composer (sidecar
    completions); click an item to insert it. `@` paths complete too.
16. Agent/model pills -> popovers over the live sidecar catalogs; picking
    calls set_agent/set_model (status line updates after re-announce).
    Guided style: starter chips fill the input; big "Send to {puppy} ->".
17. Transcript: your turns show a person avatar + "you"; puppy turns show
    the dog avatar + "{puppy}  agent · model" tag with markdown bodies
    (headings, bullets, `inline code`, fenced blocks with language tag);
    tool output renders as chips; an edit renders a chip with "+A −D" whose
    click expands green/red/dim mono diff rows (collapsed by default);
    thinking folds toggle open/closed. Long histories: only the latest 120
    entries render, with "Show older".
18. Explorer: the left tree lists the workspace (dirs first, lazy expand,
    dotfiles hidden); the Changes list at the bottom shows per-file
    +adds/−dels as the agent edits. The ▤ toggle collapses the panel.
19. Terminal: N/A on this branch by decision — "Terminal: egui branch
    only" (see GPUI_NOTES.md). The Classic composer row says so.

## 3. The Den

20. Start a relay (`cargo run -p puppy-relay`), click "Join Den" in the
    toolbar. The join card shows relay/room/name fields (relay defaults to
    `127.0.0.1:9220` or `$PUPPY_RELAY`); join a room (e.g. `qa-room-1`).
21. Header: LIVE blinks (freezes under reduce-motion); clicking the
    room-code chip copies it + toasts; "+ Invite" copies a shareable
    room+relay line; relay address shows mono on the right.
22. Join from a second instance/terminal with a different name. Both
    sides: member list grows, a system feed entry narrates the join, each
    member gets a distinct relay-assigned color.
23. Roster: each member shows their agent cards (state, agent · model,
    dir, tok/s, verb + file, +A −D) within ~3s of activity; the little
    sparkline appears after a few roster broadcasts. Open appears on YOUR
    cards only. Nudge posts a puppy message into the feed (+ toast).
24. Feed: human messages in owner colors; puppy messages with a colored
    left rule, "-> {puppy}" when addressed, and a review badge when
    flagged; send a message from the composer (Enter works). The feed is
    bottom-pinned; scrolling up holds your place (reversed-column model);
    long rooms render the latest 150 with "show all".
25. Board: add a card in any column (+), retitle via the card's ⋯ menu,
    Assign to me / Unassign, move across columns, delete. A second
    instance sees every change live. Owner chips take member colors;
    plan-derived cards show the plan tag.
26. Plans: in a workspace root, create a `plans.md` with `- [ ]`/`- [x]`
    lines. Board -> "Share to den" -> pick the workspace. A plan card
    appears for everyone (done rows struck, n/m counter). "Unshare my
    plan" removes it.
27. Presence: unfocus the window — the second instance shows you idle
    (heuristic: unfocused OR >5min without interaction; sent only on
    change). Refocus + click anywhere to flip back to active.
28. Leave den (header button or the tab's ) -> back to the dashboard;
    the second instance sees the leave; the Den tab disappears.

---

## Branch stats (for the Phase-3 comparison)

Recorded on macOS (Apple Silicon), 2026-06-12, commit `95f9502` + the 2.5
audit commit:

| Metric | Value |
|---|---|
| `cargo build --release` (clean, wall time) | 2m 27s (real 147.0s · user 672.5s) |
| Release binary size (`target/release/puppy-home`) | 6.8 MB |
| Branch diff vs fork point (`git diff --stat 0f00eed`) | 35 files, +12,970 / −199 (incl. this QA doc) |
| Dependency delta (Cargo.lock packages) | 630 -> 970 (**+340**, gpui's transitive tree; new direct deps: gpui @ pinned sha, futures, anyhow, unicode-segmentation) |
| Test count on branch | 168 unit/integration + 12 dock + 6 relay e2e = 186, all green |
| Compiler warnings | 0 (one pre-existing `block v0.1.6` future-incompat from `objc`, same as base) |
| Clean debug build | ~53s wall (515-crate cold) · warm incremental ~1-3s |

Branch commits: `070586d` turn-end metrics fix · `9006a70` 2.1 scaffold ·
`d318801` card-action ports · `dc3e939` 2.2 dashboard · `1c75d11` chat
shared surface · `a84a5e8` 2.3 chat · `a2005e0` answer UI · `95f9502` 2.4
den · (+2.5 audit/QA).

### Known deliberate deviations from the mocks

- Avatar ring **pulses** instead of spinning (no cheap rotation transform
  on a styled div at this gpui pin; a conic-gradient spin would need a
  custom shader path).
- Grid view uses flex-wrap `minmax(420,1fr)`-style sizing; last-row cards
  may stretch wider than a CSS grid would.
- Composer input: no soft wrap (long lines clip), no up/down cursor
  movement, no cursor blink. IME/selection/clipboard all work.
- Markdown is an in-house subset (headings/bullets/inline-code/bold/
  fences); no tables/links/images (Zed's markdown crate rejected for
  dependency weight — GPUI_NOTES.md).
- Terminal: deferred entirely (egui branch only).
- No image paste / attachments in the composer; no ＋New chat / sessions
  browser / logs panel / git view; no A/M/D markers in the file tree.
- Den: no legacy Activity status broadcast; no `.puppy/pack.json` Tier-2
  breadcrumb sync; plan cards cap at 8 checklist rows; kanban drag-drop
  deferred (menus, same as egui).
- No context-% bar on cards and cost always renders an em dash: the
  sidecar's status payload carries neither yet (backend gap, not UI —
  identical on both branches).
- Emoji render in **full color** (a GPUI capability the egui branch lacks).
