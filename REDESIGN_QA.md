# Redesign QA — `redesign/egui` (Phase 1)

A ~10-minute manual walkthrough of the Command Center redesign on the egui
branch. Prereqs: one or two folders you can open as workspaces (at least one
being a git repo with a `plans.md` is ideal), and optionally a second machine
or terminal running `puppy-relay` for the Den section (a local
`cargo run -p puppy-relay` works).

Pass criterion for every step: the described behavior, no visual glitches,
and no fan-spinning idle CPU (watch the `perf` HUD's repaint counter —
toggle it in the top menu).

## 1. Dashboard

1. Launch the app with no workspaces open. Dashboard shows the empty hint
   ("No agents running..."). With the `perf` HUD open, confirm the repaint
   counter is NOT climbing while you don't move the mouse (idle invariant).
2. Open a folder (top menu, "Open Folder..."). A card appears: dir-name
   title, `agent · ~path` mono meta line, model pill, "Resting" state.
3. Send a prompt from the workspace chat, return to the Dashboard. The card
   goes live: pulsing state dot, spinning avatar ring, pack-vocabulary state
   (Fetching / Sniffing / Digging) + current tool, elapsed clock ticking,
   tok/s + sparkline filling in, last-prompt inset shows your prompt.
4. Header: lede counts update ("1 on the hunt"); Throughput tile shows the
   aggregate sparkline. Spend tile shows an em dash (never $0.00).
5. Card actions while live: Pause -> state flips to "Napping" (amber), toast
   appears; Resume -> live again; Steer -> inline input expands, type a
   nudge, toggle "queue" then send -> last-prompt tag shows "+1 queued";
   send another with "now". Stop -> turn cancels, card returns to Resting.
6. Model pill -> popover lists the sidecar's models; pick one -> toast, and
   the pill + chat status line update after the sidecar re-announces.
7. Make the agent ask you something (e.g. prompt "ask me a question before
   doing anything"). Card turns pink "Needs you" + the attention banner
   appears with the question and an "Answer {dir} ->" button that jumps to
   the chat.
8. Kill the sidecar process externally (or stop the relay-less remote). The
   card shows "Stuck" red with a Restart action; Restart revives the
   session.
9. Segmented control: Grid -> List (dense table, same data + Open/Changes/
   close) -> Focus (single centered column, max 880px). Restart the app:
   the chosen view is remembered (session.json).
10. With every workspace idle/closed and the window focused, confirm again:
    repaint counter parked (no live agents = no animation repaints).

## 2. Workspace Chat

11. Open a workspace chat with no conversation ("+ New chat" if needed).
    The empty state shows: breathing puppy, floating z z z, "How can {puppy}
    help you?". Unfocus the window -> bob stops (repaint counter parks).
12. Top menu: toggle "calm" (reduce motion). The bob, dashboard pulses,
    avatar rings, and the Den LIVE blink all freeze; repaints stop while
    idle. Toggle it back. Restart the app — the calm setting survives.
13. Composer dock: status line reads "Ready · {agent} · {model}" with a
    state-colored dot; while a turn runs it shows Working/Thinking plus the
    now/queue toggle, pause/resume, and stop on the right (in every style).
14. Footer gear ("Composer: Classic") -> popover lists Classic / Unified /
    Palette / Guided with descriptions. Switch through all four; the input
    draft survives style switches (one shared state). Restart the app —
    the chosen style is remembered and applies to all workspaces.
15. Per style, send a message:
    - Classic: Commands menu, Image/Files buttons, Enter sends; second row
      has Terminal/Sessions + agent/model switcher pills.
    - Unified: accent-border bar; Enter sends, Shift+Enter newlines; typing
      "/" opens the slash palette ABOVE the bar; @ File and Paste buttons.
    - Palette: mono prompt; Cmd/Ctrl+K seeds "/" and opens the palette;
      keyboard-hint row below.
    - Guided: starter chips send immediately; labeled "Who should help?" /
      "Brain" selectors; big "Send to {puppy} ->" button.
16. Agent/model pills (any style) -> popover with two-line description rows;
    picking calls set_agent/set_model live (status line updates).
17. Transcript: your turns show a person avatar + "you"; puppy turns show
    the dog avatar + name + "agent · model" tag; tool output renders as
    chips; an edit renders a chip with "+A −D" and a collapsed "view diff"
    that opens into green/red/dim mono rows.
18. "+ New chat" clears the transcript (empty state returns) and resets the
    sidecar session.

## 3. The Den

19. Start a relay (`puppy-relay`), open the Den from the top menu. The join
    card shows relay/room/name fields; join a room (e.g. `qa-room-1`).
20. Header: LIVE blinks (stops under "calm"); clicking the room-code pill
    copies it + toasts; "+ Invite" copies a shareable room+relay line;
    relay address shows mono on the right.
21. Join from a second instance/machine with a different name. Both sides:
    member list grows, system feed entry narrates the join, each member has
    a distinct relay-assigned color.
22. Roster: each member shows their agent cards (state, model, tok/s, verb +
    file, +A −D) within ~3s of activity; sparkline appears after a few
    roster broadcasts. Open works on YOUR own cards only (teammates' show a
    disabled tooltip). Nudge posts a puppy message into the feed.
23. Feed: human messages in owner colors; puppy messages with the left color
    rule and "-> {puppy}" when addressed; send a message from the composer;
    scroll up — new entries must NOT yank you down; scroll to bottom —
    auto-stick resumes.
24. Board: add a card in Backlog (+), retitle it, assign it to a member
    (its puppy chip takes their color), move it across columns via the card
    menu, delete one. Second instance sees every change live.
25. Plans: in a workspace root, create a `plans.md` with `- [ ]`/`- [x]`
    lines. Board -> "Share plans.md" -> pick the workspace. A plan card
    appears for everyone (done rows struck). "Unshare mine" removes it.
26. Presence: leave the app unfocused ~10s — your presence dot should NOT
    flip yet; presence flips to idle when the window stays unfocused or
    after 5 minutes without input (heuristic; to test quickly, unfocus and
    watch the second instance show you idle, refocus + move the mouse to
    flip back to active).
27. Leave den -> back to the join card; the second instance sees the leave.

---

## Branch stats (for the Phase-3 comparison)

Recorded on macOS (Apple Silicon), 2026-06-12, commit `f9707a5`:

| Metric | Value |
|---|---|
| `cargo build --release` (clean, wall time) | 1m 07s (real 67.4s · user 323.2s) |
| Release binary size (`target/release/puppy-home`) | 16 MB |
| Branch diff vs fork point (`git diff --stat 0f00eed`) | 29 files, +4,902 / −1,412 |
| Dependency delta (Cargo.toml / Cargo.lock) | none — zero new dependencies |
| Test count on branch | 160 unit/integration + 12 dock + 6 relay e2e, all green |
| Compiler warnings | 0 |

Branch commits: `122f660` widget kit · `514fc17` dashboard · `363f1b4`
workspace chat · `c9fa751` den · `f9707a5` polish/QA.

### Known deliberate deviations from the mocks

- Card "glow" is a flat translucent tint under the content, not a radial
  gradient (egui has no native radial fills; per EGUI_GUIDE.md).
- Card shadows approximate the CSS values (egui 0.34 shadows have unsigned
  spread, so the `-14px` spread is emulated with smaller blur/offset).
- Emoji render monochrome (egui rasterizes outlines; accepted by the
  handoff).
- Header stat tiles sit right of the H1 and can clip on very narrow
  windows instead of wrapping under it.
- Dashboard List view keeps the close (X) action from the old table.
- Unified composer: attachment thumbnails render above the bar rather than
  as in-bar chips; @files insert text, not chip objects.
- Guided composer's drop zone is paste/click only (no OS drag-drop decode).
- Palette composer binds Cmd/Ctrl+K only (no ⌘J/⌘M); the pills cover
  agent/model switching.
- Den: no read-along of teammates' agents (Open disabled with a tooltip);
  kanban drag-and-drop deferred (menus instead); the LIVE blink keeps
  running in a quiet-but-joined room unless "calm" is on — it is the
  room's liveness signal.
- No context-% bar on cards and cost always renders an em dash: the
  sidecar's status payload carries neither yet (backend gap, not UI).
