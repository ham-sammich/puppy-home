# puppy-home
Testing
A native **windowed** AI agent application written in Rust — an IDE-like shell
around **Code Puppy**
([mpfaffenberger/code_puppy](https://github.com/mpfaffenberger/code_puppy)),
which is the real engine driving it.

The Rust app is the GUI and process supervisor; the actual agent — every tool,
agent, MCP server, skill, and model — is the genuine Python `code-puppy`
package, run in a sidecar and driven over a JSON protocol. That means we get
**all of Code Puppy's features** and stay **up to date** without reimplementing
anything.

It is **multi-workspace**: each folder you open runs its own Code Puppy process
(its own working dir, agent, model, and conversation), shown as dockable tabs,
with a **Dashboard** aggregating every running instance.

## Architecture

```
 puppy-home (Rust / egui + egui_dock)          one sidecar per workspace
 ┌──────────────────────────────────┐   JSON   ┌──────────────────────────┐
 │  Supervisor + WorkspaceMap        │ ◀──────▶ │ Code Puppy (folder A)     │
 │   DockArea: Chat │ Dashboard      │ ◀──────▶ │ Code Puppy (folder B)     │
 │   per-workspace agent/model picks │          │  cwd=folder, own agent…   │
 └──────────────────────────────────┘          └──────────────────────────┘
```

- **`sidecar.py`** imports Code Puppy, marks itself as the active renderer on
  Code Puppy's `MessageBus` (and legacy `MessageQueue`), and forwards every
  structured message (assistant text, tool output, diffs, shell output,
  confirmations, input requests) as JSON. It runs the agent via
  `get_current_agent().run_with_mcp(prompt)`, and exposes agent/model catalogs +
  switching. stdout is reserved for the protocol; stray output goes to stderr.
- **Rust `backend`** spawns one sidecar **per workspace** (with the folder as
  `cwd`) and turns its stdout into a stream of `UiEvent`s. The `supervisor`
  drives all sidecars and folds events into per-`workspace` state (including a
  derived `InstanceStatus` for the dashboard). The `shell` hosts an `egui_dock`
  `DockArea` of `Tab`s. The sidecar source is embedded via `include_str!`.

## How Code Puppy is located ("detect, else auto-provision")

On launch the app picks a launch command in this order:

1. **`PUPPY_HOME_CP_CMD`** env var (whitespace-split; the sidecar path is
   appended). Escape hatch / dev override.
2. **An existing install** — the first of `python` / `python3` / `py` on PATH
   that can already `import code_puppy`.
3. **Auto-provision with `uv`** — `uv run --with code-puppy python <sidecar>`,
   which fetches and caches Code Puppy on first run.

If none apply (no install, no `uv`), the app opens and shows an actionable error
instead of failing to start.

### Pointing at a local Code Puppy checkout (dev)

```powershell
$env:PUPPY_HOME_CP_CMD = "D:\dev\code_puppy\.venv\Scripts\python.exe"
cargo run
```

(First `uv sync` the checkout so its `.venv` exists.)

## Run

```sh
cargo run
```

A window opens with a **Dashboard** tab. Click **📁 Open Folder…** to start a
workspace: a Code Puppy process spawns scoped to that folder and a chat tab
appears. Each chat tab has its own **agent** and **model** dropdowns; the
**logs** toggle shows that workspace's sidecar stderr. Open more folders for
more workspaces — drag/split the tabs IDE-style; the Dashboard lists them all.

Dev convenience: set `PUPPY_HOME_OPEN=<folder>` to auto-open a workspace on
launch (handy for testing).

> **Note:** answering prompts requires Code Puppy to have a working model + API
> key configured (in `~/.code_puppy/puppy.cfg`), exactly as the standalone
> `code-puppy` CLI does. A `403 invalid_api_key` in the logs means that config
> needs attention, not the bridge.

## Layout

| Path                  | Role                                                          |
| --------------------- | ------------------------------------------------------------ |
| `src/main.rs`         | Window setup + launch.                                        |
| `src/app.rs`          | Top-level `eframe::App`: hosts the `egui_dock` `DockArea`.    |
| `src/supervisor.rs`   | Owns all workspaces; spawns sidecars; drains events.         |
| `src/workspace.rs`    | Per-workspace state, status FSM, and the chat view.          |
| `src/shell/mod.rs`    | `Tab`, the `TabViewer`, deferred `ShellAction`s.             |
| `src/views/`          | Cross-workspace views (e.g. `dashboard`).                    |
| `src/backend/mod.rs`  | One-sidecar handle: provisioning, spawn, JSON protocol.      |
| `sidecar/sidecar.py`  | Python bridge: Code Puppy ⇆ JSON over stdio.                 |

## Slash commands & completion

Code Puppy's commands work natively, so CLI users feel at home:

- **Type them** in the chat box — anything starting with `/` is dispatched
  through Code Puppy's real command handler; output streams into the transcript.
- **Inline autocomplete** — as you type, a CLI-style popup appears. It reuses
  Code Puppy's *own* `prompt_toolkit` completers, so it covers everything the
  CLI does and stays in sync:
  - `/` → every command (built-in **and** plugin/custom), with aliases.
  - `/agent ` → agent names · `/model ` → model names · `/set ` → config keys
    (with current values) · `/cd ` → directories · `@` → file paths · and more.
  - Navigate with ↑/↓, accept with Tab or Enter, dismiss with Esc (or click).
- **Commands ▾ menu** (top bar) — the full categorized catalog for browsing;
  picking one drops it into the composer.

Commands work identically whether typed (`/help`) or picked from the menu (the menu
just drops arg-taking commands into the composer for you). `/resume` (and
`/autosave_load`) open the **Sessions** browser instead of a terminal-only picker.

## Your puppy

You interact with **your own puppy, which has a name** (Code Puppy's global
`puppy_name`). The chat shows it everywhere — `🐶 <name>:` on replies, the composer
hint, and a **🐶 <name>** button in the toolbar you can click to **rename** it.

**Agent** and **model** have native dropdowns in each chat's toolbar (no need
for the terminal-only `/agent` / `/model` pickers). The **Commands** menu is
"smart": arg-less commands run on click; commands that need input are dropped
into the composer for you to complete. Argument forms (`/set <key> <value>`,
etc.) and `@`/`/` completion still work for CLI muscle memory.

## Interactive questions

Code Puppy's `ask_user_question` tool normally drives a terminal TUI and bails
out unless `stdin` is a real TTY — so it can't work through our headless bridge.
The sidecar installs a **connector** that monkeypatches the tool's
implementation (`registration._ask_user_question_impl`) to instead emit an `ask`
request over the protocol and block until the GUI answers. The GUI renders a
**modal** with radio options (single-select), checkboxes (multi-select), and an
"Other" field per question; Submit/Cancel sends the answer back, and the agent
continues. Simple bus-based prompts (input / confirm / select) are handled inline
in the composer.

## IDE layout (per workspace)

Each workspace tab is a small IDE:

```
┌ toolbar: 🗂 Tree · 🌿 Git · status · logs ┐
├──────────┬──────────────────────────────────────────────────────────────────┤
│  🗂 file  │  [main.rs ✕] [Cargo.toml ✕] [📝 Changes]      ← editor tabs        │
│   tree   │  path · 💾 Save                                                    │
│  (left,  │  …editable code / colored diff…                ← editor (top)      │
│ toggle)  ├──────────────────────────────────────────────────────────────────┤
│          │  …chat transcript…                             ← chat (bottom,     │
│          │  ⌘ Commands ▾  Message Code Puppy…       [Send]    resizable)       │
│          │  🖥 Terminal · 🐶 Agent ▾ · Model ▾             ← bottom menu bar   │
│          │  (terminal on = a full PTY grid fills the chat area)               │
└──────────┴──────────────────────────────────────────────────────────────────┘
```

The **🖥 Terminal** toggle (bottom bar) swaps the chat area for a **full
pseudo-terminal** — a real `powershell` (or `$SHELL`) on a PTY (ConPTY on
Windows) rooted at the workspace folder. Output runs through a `vt100` screen
parser and is drawn as a real cell grid, so **colors, cursor movement, and
curses-style TUIs (vim, top, htop) work**. Click to focus and type straight into
it (Ctrl+C interrupts, arrows/Tab/etc. are forwarded); mouse-wheel scrolls
back through history; **⟳** restarts it; it auto-resizes to the panel. The shell
is `powershell` on Windows and `$SHELL` (zsh/bash) on macOS/Linux — override with
`PUPPY_HOME_SHELL`.

> **Cross-platform:** puppy-home targets Windows, macOS, and Linux. The terminal
> uses a native PTY (ConPTY / openpty) and the UI loads per-OS system fonts at
> runtime, so nothing is hard-coded to one platform.

- **🗂 Tree** (toggleable) lists the workspace folder; noisy dirs
  (`target`/`.git`/`node_modules`/…) are hidden, folders lazy-expand. Changed
  files show an inline marker (**A/M/D**, `?` untracked).
- A **Changes** panel (Source-Control style) sits at the **bottom of the tree**.
  In a **git** workspace it shows the real working-tree status (staged, unstaged,
  *and locally-made* changes — not just Code Puppy's), refreshed on a background
  thread (and after each AI edit); a **⟳** button forces a refresh. In a non-git
  folder it falls back to the changes Code Puppy reports. Click a file to see its
  diff.
- Clicking a file opens it as an **editor tab** *inside the workspace* (above the
  chat) — a **syntax-highlighted** code editor (via `egui_extras`/syntect) with
  **💾 Save** / Ctrl+S, dirty marker (●), per-file buffers. When Code Puppy edits
  an open file, the buffer **reloads from disk** automatically (unless you have
  unsaved edits). Diffs open in a **Changes** editor tab.
- The **editor/chat divider is draggable** (the resize hit-zone is widened so
  it's easy to grab).
- With files open, the **chat is pushed into a resizable bottom panel**; with
  nothing open, the chat fills the area.

## Rendering

- **Markdown** — agent responses are rendered as formatted markdown
  (headings, lists, tables, blockquotes, **syntax-highlighted** code fences) via
  [`egui_commonmark`](https://crates.io/crates/egui_commonmark), not raw source.
- **Fonts / Unicode** — egui only bundles Latin plus a small emoji subset, so
  [`src/fonts.rs`](src/fonts.rs) registers fallbacks: a bundled **full monochrome
  Noto Emoji** (`assets/NotoEmoji-Regular.ttf`), **Segoe UI** (+ Symbol), and the
  Windows **CJK** fonts (YaHei / Yu Gothic / Malgun Gothic) so 中文 / 日本語 /
  한국어, symbols, and all emoji resolve instead of showing as boxes. Note: egui
  rasterizes glyphs monochrome — emoji are silhouettes, not color.

## Protocol (summary)

GUI → sidecar (stdin): `prompt`, `cancel`, `command`, `complete`,
`list_commands`, `list_agents`, `list_models`, `set_agent`, `set_model`,
`ask_response`, `respond_input`, `respond_confirmation`, `respond_selection`,
`shutdown`.

sidecar → GUI (stdout): `ready` (with `cwd`), `commands`, `agents`, `models`,
`completions`, `ask`, `message`, `result`, `command_done`, `error`, `log`.

## Roadmap

Done: multi-workspace dockable shell · per-workspace agent/model pickers ·
smart commands · basic Dashboard · interactive-question modal · markdown +
emoji/Unicode rendering · async folder picker · File & Diff view · IDE layout
(editor above, resizable chat below) · file-tree explorer · editable file tabs
with **syntax highlighting**.

- [x] **Git view** — a Source Control page (toolbar **🌿 Git**, in a git
      workspace): current branch + ahead/behind, a commit box, staged vs unstaged
      lists with per-file and bulk stage/unstage, **Commit**, and a clickable
      history (each commit opens its patch). Plus a **🔍 Blame** toggle that
      annotates the file you're viewing in place — each line gets its commit /
      author / date in a gutter, syntax-highlighted and read-only, toggled back
      off to edit. Backed by [`src/git.rs`](src/git.rs) shell-outs; working-tree
      status + diffs already drive the Changes panel.
- [x] **Dashboard: tokens & sub-agent rows** (via a `status` op) — while a turn
      runs the GUI polls a `status` op (~every 1.2 s) and the dashboard shows the
      token-generation rate, conversation stats (avg TTFT / throughput, on hover),
      and a nested row per concurrent sub-agent (`invoke_agent`) with its
      model, status, current tool, tool count, tokens, and elapsed — alongside
      the existing live state, tool-call count, and attention badges (tab `●`,
      top-bar + dashboard "waiting for input").
- [x] **Pause/steer a running turn** — while a turn runs the composer shows
      **⏸ Pause/▶ Resume** (halts/continues at the next safe boundary) and a
      **Steer** box: type and send a mid-turn instruction without cancelling.
      A **🎯 now / 📨 queue** toggle picks delivery — *now* interrupts at the next
      model call (even between tool calls), *queue* lands after the current turn.
      Drives Code Puppy's own `PauseController` + steer history-processor.
- [ ] Color emoji (image-based, e.g. egui-twemoji) instead of monochrome.
- [x] **Persist/restore workspaces & sessions** — the folders you have open (with
      each one's agent, model, **and Code Puppy autosave session**) are saved to a
      per-user `session.json` and reopened on the next launch, **resuming each
      workspace's conversation** where you left off. (`PUPPY_HOME_OPEN` still
      force-opens an extra folder for dev.)
- [x] **Session browser** — a **🗂 Sessions** button (bottom bar; also `/resume`)
      opens an interactive two-pane picker: a list of every saved Code Puppy
      conversation (autosave + named contexts, with message/token counts) on the
      left, and a **read-only preview of the selected session's conversation** on
      the right. **Resume this** loads it into the current workspace (the agent
      reloads that history; the transcript is reconstructed). Each workspace stays
      tied to its autosave session, so new turns keep extending it across runs.
```

## Troubleshooting

### NVIDIA overlay pops up when puppy-home launches (Windows)

puppy-home renders through the GPU (an OpenGL window via eframe/glow), and
NVIDIA's in-game overlay auto-attaches to **any** process that creates a GPU
context — it assumes the app is a game. NVIDIA ships no API for an app to opt
itself out, so this is controlled on your machine, not in our code:

- **NVIDIA App** (current): Settings → **Overlay** → toggle off — or keep the
  overlay for games but go to Overlay → Settings and disable **notifications**,
  which silences the "Press Alt+Z" toast on launch.
- **GeForce Experience** (legacy): Settings → General → **IN-GAME OVERLAY** →
  off.
- The overlay is harmless to puppy-home either way; disabling it only removes
  the popup/injection. The same applies to Discord/Steam overlays if they ever
  hook in — disable those per-app in their own settings.
