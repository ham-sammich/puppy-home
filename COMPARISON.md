# egui vs GPUI — the Command Center redesign, built twice

Phase 3 of the redesign initiative. The same design (Dashboard / Workspace
Chat / The Den, amber brand, live data) was implemented **in full, twice**,
on two branches forked from the same shared backend (`redesign/shared-backend`
@ `0f00eed`):

- **`redesign/egui`** — on the existing eframe/egui 0.34 stack (Phase 1)
- **`redesign/gpui`** — replacing eframe with GPUI, pinned to Zed
  `v0.199.10` / `00789bf6ee74` (Phase 2)

This file is committed **identically to both branches**. Numbers come from
each branch's `REDESIGN_QA.md` (same machine: macOS, Apple Silicon,
rustc 1.96.0, 2026-06-12).

---

## Executive summary

*This is the implementing agent's opinion, clearly labeled as such — the
humans make the call.*

> Both branches shipped the design with live data, and the honest gap
> between them is smaller than the framework debate suggests. **egui won on
> cost**: zero new dependencies, a 1-minute clean build, and an embedded
> terminal that GPUI never got; its ceiling shows in monochrome emoji,
> hand-rolled animation timing, and the restyle-everything nature of
> immediate mode. **GPUI won on ceiling**: real animations, color emoji,
> CSS-grade styling, and a component model that made the Den and the
> dashboard cards genuinely pleasant to build — but it cost +340 transitive
> dependencies on an unstable pinned sha, a 2.4x clean-build time, and we
> had to hand-build a text input that egui gives away for free. My read:
> if puppy-home's identity is "lean tool that builds fast everywhere",
> stay egui (a). If it's "the polished AI-coding cockpit the mocks
> promise" and the team can absorb chasing Zed's API a few times a year,
> migrate (b) — the backend seam (`UiWaker` + frontend-agnostic
> `Workspace` methods) built for this comparison makes either choice, or
> deferring it (c), cheap to revisit.

---

## 1. Fidelity vs the mock, per surface

Rough %-of-mock judgments by the implementer; see each branch's
`REDESIGN_QA.md` "known deviations" for the precise ledger.

### Dashboard ("Running agents")

| | egui | gpui |
|---|---|---|
| Fidelity | ~90% | ~92% |
| Card anatomy (avatar/dot/pill/state/prompt/stats/subs/actions) | full | full |
| Grid / List / Focus + persistence | full (computed columns) | full (flex-wrap; last row may stretch) |
| Animation | pulse + ring via repaint scheduling | native repeating animations; entrance fade |
| Named gaps | flat glow (no radial), approximated shadows, mono emoji | ring pulses instead of spinning, flex-wrap grid quirk |
| Shared backend gaps (both) | no ctx-% bar, cost renders em dash | same |

### Workspace Chat

| | egui | gpui |
|---|---|---|
| Fidelity | ~85-90% | ~80% |
| Composer styles (4 over one state, persisted) | full | full |
| Input | egui TextEdit (free: wrap, undo, IME) | hand-built EntityInputHandler (IME/selection/clipboard; **no soft wrap**, no up/down) |
| Slash palette / switchers | full, keyboard nav | completions palette click-to-insert (no arrow-key nav) |
| Transcript | commonmark markdown, chips, lazy diffs, 120 tail | in-house markdown subset, chips, lazy diffs, 120 tail |
| Ask-answer ("Needs you") | modal (shared pipeline) | inline panel (same shared pipeline) |
| Named gaps | attachment chips above bar, no drag-drop decode | no image paste, no +New chat / sessions / logs / git view, no tree A/M/D markers |
| Terminal | **embedded PTY terminal works** | **N/A — deferred by decision** |

### The Den

| | egui | gpui |
|---|---|---|
| Fidelity | ~85% | ~85% |
| Header / roster / board / plans / feed / presence | full | full |
| Roster sparklines (derived from broadcasts) | yes | yes |
| Named gaps | no teammate read-along, kanban menus not drag-drop | same two, plus: no legacy Activity broadcast, no `.puppy/pack.json` Tier-2 breadcrumb, plan cards cap 8 rows |

## 2. Engineering numbers

| Metric | redesign/egui | redesign/gpui |
|---|---|---|
| Diff vs `0f00eed` | 32 files, +5,189 / −1,469 | 35 files, +12,970 / −199 |
| Clean release build (wall) | **1m 07s** | 2m 27s |
| Clean debug build (wall) | (not recorded; sub-minute) | ~53s (515-crate cold) |
| Warm incremental | ~1s | ~1-3s |
| Release binary | 16 MB | **6.8 MB** |
| New direct deps | **0** | gpui (git pin), futures, anyhow, unicode-segmentation |
| Cargo.lock packages | 630 (unchanged) | **970 (+340)** |
| Tests green | 178 (160+12+6) | 186 (168+12+6) |
| Warnings | 0 | 0 |

Notes: the egui binary is larger because eframe/glow stays linked and used;
on the gpui branch the dormant egui code is linker-GC'd. LOC delta
overstates gpui's "cost" somewhat — it includes a ~760-line text input and
~250-line markdown renderer that egui gets from crates, plus this branch
kept (rather than rewrote) every legacy module.

Dependency **risk** is the asymmetric one: gpui is not on crates.io; the
pin is a git sha into the Zed monorepo. Every future bump is a manual
"chase the API" event (the GPUI_GUIDE predicted this; signatures were
already drifting from its sketch at our pin). egui's 0.x releases also
break APIs, but on crates.io cadence with migration notes and a far
smaller blast radius.

## 3. Capability asymmetries

- **Terminal**: egui branch has the working embedded PTY/vt100 terminal
  (pre-existing code). GPUI got none — porting the grid as a custom
  Element is feasible (we now know the Element API well) but unbudgeted.
  Zed's terminal crates exist but drag heavy deps. **Advantage egui
  (today), gpui (eventually, via Zed's own terminal).**
- **Color emoji**: GPUI renders the puppy  in color; egui rasterizes
  monochrome outlines. For a brand built on a dog emoji this is more
  meaningful than it sounds. **Advantage gpui.**
- **Animation quality**: egui animations are hand-scheduled repaints —
  fine for pulses, but every animation is bespoke timing math and a
  repaint-budget worry. GPUI's `with_animation` is declarative
  (duration/easing/repeat), composes with styling, and only invalidates
  the animated element. The entrance fades and LIVE blink took minutes,
  not hours, and feel native. What GPUI *couldn't* do cheaply at our pin:
  rotation (the spinning avatar ring became a pulse). **Advantage gpui,
  clearly.**
- **Input maturity**: egui's `TextEdit` ships wrap/undo/IME/selection for
  free. GPUI at this pin has no input widget — we ported and extended the
  ~700-line example (`EntityInputHandler`) to get IME + selection +
  multiline, and it still lacks soft wrap and vertical cursor movement.
  This was the single largest GPUI line-item. **Advantage egui.**
- **IME**: both work; gpui's marked-text protocol is the real macOS one
  (we render underlined composition); egui's is adequate. Tie-ish.
- **Popovers / z-order**: egui popovers fight the immediate-mode painter
  order (the egui branch used Areas/Order juggling). GPUI's
  `deferred().with_priority()` + `occlude` + `on_mouse_down_out` is a
  clean, local idiom — popovers were one of the *easiest* GPUI parts.
  **Advantage gpui.**
- **Layout**: GPUI's flexbox (Taffy) maps 1:1 from the HTML mocks; the
  egui branch hand-computed column math. **Advantage gpui.**

## 4. Maintenance outlook

- **Changing things**: egui is restyle-everything-per-frame — tweaks are
  fast and local, but every visual nicety (shadows, glows, pulses) is
  custom paint code in the widget kit, and complex layouts mean manual
  math. GPUI reads like the CSS the designers wrote; a styling change is
  usually one builder-chain edit. Conversely, egui has no borrow-checker
  friction; GPUI's closure-heavy API forced the snapshots-down/actions-up
  discipline (which we'd now call a feature — see GPUI_NOTES.md).
- **Structure**: the egui branch's chat lives in `view.rs`-style splits of
  one `Workspace` god-object render path; the gpui branch is one RootView
  entity + stateless RenderOnce components fed snapshots, with a single
  `dispatch` mutation funnel. The gpui shape is the one we'd want to grow.
- **API stability / docs / community**: egui — stable-ish 0.x on
  crates.io, excellent docs, big community. gpui — the Zed source IS the
  documentation, no semver, smaller community; we verified every
  signature against the pinned checkout. Budget real time per pin bump.
- **The escape hatch**: the shared backend is now genuinely
  frontend-agnostic (UiWaker, card-action senders, ask pipeline, one
  session.json). Whichever branch loses can be deleted without touching
  the 60% that matters.

## 5. Performance characteristics observed

- **egui**: immediate mode repaints on a schedule; the branch's discipline
  (event-driven repaints, animation gating, 120-entry transcript tail) was
  audited to "idle = parked repaint counter". Its perf HUD made this
  verifiable in-app.
- **gpui**: retained — idle means *zero* render work by construction; the
  drain loop polls the backend at 250ms busy / 1s idle and `cx.notify()`s
  only then; animations invalidate only their elements and exist only on
  the rendered screen. Observed across all probes: hours of cumulative
  live runs (sidecar turns, ask waits, relay rooms with a second member)
  with no panic and no runaway repaint behavior. Not measured: frame
  times under load (no HUD on the gpui branch — a known gap).
- Both branches share the same backend costs (status polling, spark
  rings, diff parsing made lazy on both).

## 6. Recommendation paths

- **(a) Stay egui** — right if: build time, dependency count, and the
  embedded terminal matter more than polish ceiling; the team wants
  crates.io-grade stability. Cost: the mock's motion/emoji/gradient
  finesse stays approximated; chat/editor sophistication stays bounded by
  immediate-mode budgets.
- **(b) Migrate to gpui** — right if: the product bet is "most polished
  AI-coding cockpit", color emoji + native motion are brand-level
  requirements, and the team accepts: chasing a pinned sha (plan ~1-2
  days per bump, a few times a year), porting/adopting a terminal, and
  finishing the input (soft wrap, vertical movement) or adopting Zed's
  editor crate. The architecture patterns are proven and documented.
- **(c) Hybrid / wait** — keep shipping on egui; keep the gpui branch
  rebased quarterly as a tracking spike; revisit when gpui stabilizes
  (crates.io release or API churn slows). Cost: double-maintenance of two
  UI trees for anything user-facing; the shared-backend seam keeps that
  cost real but bounded.

What would have to be true: (a) needs nobody to care that the app looks
"egui-good" rather than "Zed-good"; (b) needs a terminal answer and a
pin-bump budget; (c) needs discipline to actually rebase the spike, or it
rots.

## 7. Appendix

### Commit logs (fork point `0f00eed`)

**redesign/egui**: `122f660` widget kit · `514fc17` dashboard · `363f1b4`
chat · `c9fa751` den · `f9707a5` polish · `cb835c6` QA · `7827db1`
turn-end metrics (cherry-pick) · `9933a3e` shared ask pipeline
(cherry-pick).

**redesign/gpui**: `070586d` turn-end metrics fix · `9006a70` 2.1 scaffold
(pin + waker + drain loop) · `d318801` card-action ports · `dc3e939` 2.2
dashboard · `1c75d11` chat shared surface + 2-arg prompt convergence ·
`a84a5e8` 2.3 chat · `a2005e0` ask-answer UI · `95f9502` 2.4 den ·
`b648c1c` 2.5 audit + QA.

### QA scripts

Each branch carries its own `REDESIGN_QA.md` (~10-min, 27/28 steps,
numbered identically by surface) with the branch's stats table and
known-deviations ledger. The gpui branch additionally documents every
framework decision in `GPUI_NOTES.md` (pin rationale, waker/drain
pattern, snapshot/dispatch architecture, markdown + terminal decisions,
input design, den glue).

### Known-gaps ledger (the union, attributed)

- Both: no ctx-% bar, cost always em dash (sidecar payload gaps), kanban
  drag-drop (menus instead), teammate read-along not wired.
- egui only: mono emoji, flat glow/approx shadows, palette composer binds
  only Cmd/Ctrl+K.
- gpui only: no terminal, no image paste, no +New chat / sessions / logs /
  git view, input lacks soft wrap, in-house markdown subset (no
  tables/links), no legacy Activity broadcast / pack.json breadcrumb,
  ring pulses instead of spins.

### Open items needing a human decision

1. **The framework call itself** (this document's reason to exist).
2. If (b): terminal strategy — port our vt100 grid vs adopt Zed's crates.
3. Sidecar backend gaps both branches inherit: ctx-% + cost ledger in the
   status payload (would light up the dormant UI on both branches).
4. Whether the egui branch should adopt the gpui branch's
   `Session.composer_style`-style read-modify-write prefs saving wholesale
   (the gpui branch already carries egui fields safely; reverse direction
   is wired via dock_layout carry).
5. Neither branch is pushed to origin — pushing/PRs are a human call.
