# WINDOWS_GATE.md — G3 smoke gate for `redesign/gpui`

## 1. Context

`puppy-home` (a desktop command center for code_puppy AI agents) was
rebuilt from egui onto **GPUI** (Zed's UI framework, pinned at rev
`00789bf6ee74` ~ v0.199.10, frozen). All development and testing so far
happened on macOS; this branch has **never been built or run on
Windows** (CI's windows-latest leg only fires on master/PRs — never on
this branch). Windows pass = the merge blocker (G3 in PARITY.md).
Fill the RESULTS template at the bottom; a reviewing agent with no
other context will parse it.

## 2. Prereqs

- **Rust**: stable toolchain via rustup (branch is built with rustc
  1.96.0; any current stable should do), **MSVC target**
  (`x86_64-pc-windows-msvc`, the rustup default on Windows).
- **MSVC build tools** (Zed's documented requirements at our pin):
  - Visual Studio or Build Tools with *Desktop development with C++* —
    components `MSVC v143 C++ x64/x86 build tools` **and the
    Spectre-mitigated libs variant** (Zed's docs list
    `Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre`).
  - **Windows SDK >= 10.0.20348.0** (Zed docs; they install
    `Windows11SDK.26100`).
  - If you used Build Tools (not full VS): build from the *Developer
    PowerShell* so the env vars are set.
  - CMake: Zed's docs require it **for wasmtime, which our tree does
    not pull** (0 wasmtime entries in Cargo.lock) — skip it unless a
    build error names it.
- **WebView2 runtime** for the browser plugin (preinstalled on Win11
  and any machine with Edge; the plugin is wry-based).
- **Sidecar**: `uv` on PATH (`winget install astral-sh.uv`) — the app
  spawns `uv run --with code-puppy python sidecar.py`; first spawn
  downloads code-puppy into uv's cache. A model credential config
  (`%USERPROFILE%\.code_puppy\`) must exist or be created on first
  run.
- **curl on PATH** for the version check (ships with Windows 10
  1803+; `curl --version` to confirm).
- **git** installed and configured (git view + graph read the repo via
  the git CLI/lib).
- Fonts: none to install — Space Grotesk, JetBrains Mono, and Noto
  Emoji are embedded in the binary.

## 3. Build

```powershell
git clone <repo-url> puppy-home
cd puppy-home
git checkout redesign/gpui
cargo build --release            # app
cargo build --release -p puppy-relay     # den relay (lands next to the app)
cargo build --release -p puppy-browser   # browser plugin
```

Expectations (macOS M-series reference: clean release 1m53s wall /
~9.5 CPU-min, 970 crates): a similar-class Windows box should land in
the **2-6 minute** range; binary `target\release\puppy-home.exe`
around **12 MB** (give or take format differences).

Known pitfalls:
- The `runtime_shaders` gpui feature we enable is **macOS-only and an
  empty feature flag in gpui's Cargo.toml** — inert on Windows. If the
  build fails mentioning it, that's a real finding; report it.
- `RUSTFLAGS=-D warnings` is NOT set locally; warnings are fine,
  errors are not.
- If linking fails with Spectre-lib errors, install the
  Spectre-mitigated libs component (above) — a known Zed-on-Windows
  trip hazard.

## 4. Smoke checklist

For every item: record PASS/FAIL in §6, capture the named evidence
(screenshot `gN.png` or pasted text), and note anything odd even on
PASS.

### A. First light

1. **Launch + render**: run `target\release\puppy-home.exe`. Window
   opens; dark amber dashboard; "Code Puppy" brand top-left; no
   panics on the console. *Evidence: g1.png (whole window).*
2. **Fonts**: headings in Space Grotesk (geometric sans, distinctive
   double-story 'g'); mono strings (`~path`, status lines) in
   JetBrains Mono; emoji visible in the identity chip + dashboard
   lede (color or mono both acceptable — note which).
   *Evidence: g2.png (toolbar close-up).*

### B. Windows-specific risks (by-construction code, priority order)

3. **Terminal / ConPTY** (portable-pty's ConPTY path + our vt100 grid
   — never run on Windows): open a workspace chat -> Terminal toggle.
   Expect `powershell.exe -NoLogo` in the chat area.
   - `dir` — output renders, colors sane
   - `cls` — clears
   - `git log` — pager scrolls, `q` exits (keys reach the PTY)
   - arrow-key history recall in PowerShell works (Up/Down)
   - type `python` if available: REPL arrows + Ctrl-C interrupt
   - wheel-scroll up -> scrollback banner; typing snaps back to live
   - resize the OS window -> shell reflows on the next frame (run
     `dir` again; no stair-stepped wrap)
   *Evidence: g3.png (terminal showing dir + a TUI/pager).*
4. **Browser plugin EMBED** (the `SetParent`-into-GPUI-HWND reparent
   path — written blind, never executed anywhere): after building
   puppy-browser, click **Web** in the toolbar. If the plugin isn't
   auto-found, the Browser screen names the scanned plugins dir —
   either copy `target\release\puppy-browser.exe` there per its
   instructions, or set `PUPPY_PLUGINS_DIR` and relaunch.
   - Embedded page renders INSIDE the app window at the canvas rect
     (not floating over other apps)
   - Address bar navigation works; the embed tracks window moves and
     resizes without lag or offset drift
   - Switch to Dashboard tab -> embed hides; back -> reappears
   - Minimize -> restore: embed follows
   - **Pop out** -> becomes a normal floating window (has its own
     title bar / can be moved independently); **pop back in** ->
     re-embeds correctly; **close (x)** -> the `puppy-browser.exe`
     process exits (verify in Task Manager)
   *Evidence: g4a.png (embedded), g4b.png (popped out), note on
   process exit.*
5. **Fractional DPI** (the repo's old egui Windows-DPI scars; GPUI's
   Windows scaling at this pin is unproven by us): Settings > Display
   > Scale = **125%**, relaunch; then **150%**, relaunch.
   - Text crisp (no blur), no clipped toolbars/cards, popovers open
     under their anchors, no creeping layout on hover
   - With the browser embedded: the embed rect still matches the
     canvas (DPI math in the reparent path is px-vs-DIP sensitive —
     watch for the page sitting offset or scaled wrong)
   - Terminal cell grid aligns (no smeared row of text at the bottom)
   *Evidence: g5a.png @125%, g5b.png @150% (browser + terminal each
   visible in at least one).*
6. **ssh exec + quoting** (remote workspaces shell out to `ssh.exe`;
   arg quoting was written against OpenSSH semantics): requires
   Windows' built-in OpenSSH client (`ssh -V`) and any reachable
   host with key auth (vm840 if reachable from this box).
   - Connect Remote… -> enter `user@host` -> folder browser lists
     remote dirs -> open one. If the host has uv/code_puppy: workspace
     goes Ready; run a one-line prompt.
   - **Fallback offer**: relaunch with
     `$env:PUPPY_HOME_CP_CMD="definitely-not-a-real-launcher"` and
     connect again -> the remote spawn fails and the app OFFERS
     ssh-fallback mode (local sidecar driving the remote over ssh);
     accept; the session row flags `ssh-fallback`. Unset the var
     after.
   - **Creds push**: on a remote workspace, the toolbar key icon ->
     arm -> confirm -> pushes `%USERPROFILE%\.code_puppy` credentials
     to the host (paths on the REMOTE side are POSIX — that's
     correct). Verify no credential text appears in any log/console.
   *Evidence: g6.png (remote workspace Ready or fallback-flagged row)
   + note which legs ran (skip legs honestly if no host reachable —
   mark N/A, the gate reviewer weighs it).*

### C. Core flows (quick pass)

7. **Open folder + turn**: Open Folder… -> pick a small git repo ->
   card appears -> Open -> prompt "list the files here" -> streamed
   turn completes; transcript markdown renders; tool chips show.
   *Evidence: g7.png.*
8. **Pause / steer / stop** mid-turn from the dashboard card (longer
   prompt): Pause -> "Napping"; Resume; Steer "now"; Stop.
9. **Editor**: click a file in the tree -> syntax highlighting; edit
   -> dirty dot; Ctrl+S -> saved (verify on disk); close tab.
10. **Git view**: stage/unstage the §9 edit via checkboxes; commit
    with a message; the graph shows the new commit on the branch
    lane. (Don't push.)
11. **Den self-host** (Windows pathing of the relay resolution order:
    `puppy-relay.exe` NEXT TO `puppy-home.exe` — both in
    `target\release` after §3): Join Den -> **Host a Den** -> HOSTING
    chip with `ip:port · room`; roster shows you. Then the orphan
    check: **kill puppy-home.exe from Task Manager** (End task) ->
    within ~10s `puppy-relay.exe` must disappear from Task Manager
    too (the parent-watchdog uses `tasklist` on Windows — written
    blind, this is its first run). *Evidence: note + g11.png
    (HOSTING chip).*
12. **Session restore**: with 1-2 workspaces open, quit normally,
    relaunch -> same folders reopen, agent/model reapplied, dashboard
    view + composer style + theme + avatars all remembered
    (`%APPDATA%\puppy-home\session.json`).
13. **Theme switch** Dark -> Light -> Dark: all surfaces re-skin, no
    unreadable text. **Motion: off**: pulses/bobs/blinks freeze.
14. **Managers sanity**: open MCP / Skills / Agents / Models / Config
    once each — lists populate (Config works even with no sidecar),
    no panic, overlays close.

### D. Idle discipline

15. **Task Manager CPU** with the app idle (no turn running) on each
    screen for ~30s: Dashboard, a Chat, Terminal visible, Browser tab
    (focused AND unfocused — unfocused must drop to ~0%, the RAF
    pump is focus-gated), Den. Expect ~0% (sub-1%) everywhere idle.
    *Evidence: g15.png (Task Manager while on the worst screen).*

## 5. Known N/A / expected differences on Windows

- **Relay watchdog** uses `tasklist` polling (3s) instead of
  `kill -0`; §B.11 is its first execution ever.
- **Hosting a den** binds `0.0.0.0` -> Windows Firewall will prompt
  on first run; allowing it is expected (private networks suffice).
- **Emoji** may render monochrome (embedded Noto Emoji / DirectWrite
  fallback differences) — acceptable; note which.
- **`open`-folder affordances** use `explorer` on Windows (already
  branched in code).
- **code_puppy config** lives at `%USERPROFILE%\.code_puppy` (same
  dotdir convention as Unix — matches code_puppy itself).
- **ssh-fallback scratch dir** lands under `%USERPROFILE%\.cache\
  puppy-home\ssh-fallback\` — unconventional on Windows but
  intentional (cross-platform path, sanitized slug).
- Smooth-scroll/momentum feel differs from macOS trackpads; judge
  correctness, not feel.
- The perf HUD and some probe env vars are dev-only; ignore.

## 6. Results template (fill this in)

```markdown
### Environment
- Windows version/build:
- GPU + driver:
- Display scaling tested: 100% / 125% / 150%
- rustc -V:
- uv --version / curl --version / ssh -V:
- Commit tested (git rev-parse --short HEAD):

### Results
| # | Item | PASS/FAIL/N-A | Evidence | Notes |
|---|------|---------------|----------|-------|
| 1 | Launch + render | | g1.png | |
| 2 | Fonts/emoji | | g2.png | |
| 3 | Terminal ConPTY | | g3.png | |
| 4 | Browser embed/popout/close | | g4a/b.png | |
| 5 | DPI 125%/150% | | g5a/b.png | |
| 6 | ssh remote + fallback + creds | | g6.png | legs run: |
| 7 | Open folder + turn | | g7.png | |
| 8 | Pause/steer/stop | | | |
| 9 | Editor open/save | | | |
| 10 | Git view + commit | | | |
| 11 | Den self-host + orphan check | | g11.png | |
| 12 | Session restore | | | |
| 13 | Theme + reduce motion | | | |
| 14 | Managers sanity | | | |
| 15 | Idle CPU per screen | | g15.png | worst screen + % |

### Build record
- cargo build --release wall time:
- binary size (puppy-home.exe):
- warnings/errors pasted below (if any):
```

Reviewer guidance: items 1-5 are hard blockers; 6 and 11 may be
partially N/A for environment reasons (judge from the notes); 7-15
failures block unless trivially cosmetic (then file them on the
PARITY cosmetic ledger with a screenshot).
