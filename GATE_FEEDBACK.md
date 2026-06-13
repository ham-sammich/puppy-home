# Gate-3 live-testing feedback ledger (Windows, 2026-06-12)

Source: Jacob's hands-on G3 run on Windows 11 (commit f423ba0,
redesign/gpui). Triage column says who/where it lands. This file is
the cross-device handoff — the Mac-side agent should read it too.

## Blockers (gate items)

| # | Bug | Gate item | Status |
|---|-----|-----------|--------|
| B1 | Browser works **popped out** but NOT embedded inside the app (Windows reparent path) | G3 #4 | INVESTIGATING on this box — see notes below |

### B1 notes (for whoever picks this up)
- Plugin starts borderless+hidden; host reparents via `SetParent` in
  `src/browser/embed.rs::reparent` (style swap WS_POPUP->WS_CHILD).
- Suspect 1: no `SetWindowPos(..SWP_FRAMECHANGED)` after the
  `GWL_STYLE` change (MSDN requires it for style changes to apply).
- Suspect 2: GPUI's Windows surface may be DirectComposition
  (`WS_EX_NOREDIRECTIONBITMAP`?) which can compose OVER child HWNDs —
  if so the fix is the macOS-style glued-overlay approach instead of
  reparenting.

## Bugs (non-blocking, fix on either device)

| # | Bug | Area | Notes |
|---|-----|------|-------|
| F1 | SSH remote folder browser flashes a console window on every folder click | `views/remote_connect.rs` -> `ssh.rs` | FIXED on this box: `hide_console` moved into `base_ssh()` |
| F2 | SSH connect dialog: at default window size the Connect button is off-screen after picking a folder via the browser (no scroll) | remote connect dialog layout | DONE: header + footer (Connect/Cancel) now pinned; the middle body (hosts/target/path-browser/push) is a flex_1 min_h_0 overflow_y_scroll region, so the action row is always reachable. |
| F3 | App cuts off items at the bottom of (some) views; can't scroll them into view, must resize the window | global layout | PARTIAL: the remote dialog instance is fixed (see F2). Audited all overlays — managers, theme editor, sessions, model pill, composer popovers, git panels, editor all already have scroll containers. Remaining suspects without scroll are only the small anchored popovers (about/avatars). NEED JACOB to name the specific view(s) still cutting off. |
| F4 | Workspace explorer: hidden directories (dotdirs) not shown | file tree | DONE: now shown by default + explorer header toggle cycling Show/Dim/Hide (HiddenMode in session.rs, persisted). |
| F5 | Workspace explorer: no way to create/add files or folders | file tree | DONE: per-row right-click New File/Folder/Rename/Delete were wired but undiscoverable + had no root entry point. Added visible "+file/+dir" buttons in the EXPLORER header that create at the workspace root (works in empty repos), with inline name input + cancel (TreeOpCancel). |
| F6 | Local file browser (file reference picker) can't navigate to arbitrary folders | file picker | DONE: "up" no longer gated on the workspace root (climb to drive root), + an editable path bar pinned atop the picker that reflects the current dir and jumps to any typed folder/drive (e.g. D:\) on Enter (PickerGoPath). |
| F7 | New workspace card: screen flashes on add, and the card appends to the END of the list while the eye expects top-left | dashboard | fix flash; consider insert-at-front or scroll-to-new |
| F8 | Agent creator opens a NEW WORKSPACE; should stay localized inside the agent modal ("create with agent creator" button) | agents manager | |
| F9 | Agent creator: focus is a weird shifted view | agents manager | may scrap the feature if no clean fix |
| F10 | Den: a non-host user only sees their OWN agents; the host sees everyone's. (Messages, nudges, board all work.) | relay / den sync | agent roster broadcast is asymmetric — needs two devices to test, good Mac+Win pairing task |
| F11 | Profile pfp is emoji-only; want real photo uploads | profile/identity | image picker + avatar storage + den transport |

## Research notes

| # | Topic | Notes |
|---|-------|-------|
| R1 | **In-app browser embedding in Rust apps — research better approaches.** Current state (2026-06-12) WORKS but is not ideal: separate wry/tao plugin process glued over the GPUI canvas as an owned borderless Win32 overlay (macOS: NSWindow ordered above host via IPC). | Pain points: (a) GPUI's window is a DComp surface (WS_EX_NOREDIRECTIONBITMAP) so true child-HWND embedding is invisible — that's WHY we overlay; (b) overlay z-order/focus is a simulation — clicking the page focuses another process's window; (c) drag-tracking needs an 80 Hz glue thread because Windows modal move loops starve the render loop; (d) tao's borderless = WM_NCCALCSIZE trickery — host must never touch GWL_STYLE or set_decorations desyncs. Avenues to research: WebView2 **visual hosting** (CompositionController) composed INTO the host's DComp tree (would be true in-canvas embedding; needs in-process webview or shared visuals); wry CHILD webview INSIDE our own message-loop window; servo/verso embedding; CEF offscreen rendering into a GPUI texture (heavyweight but true composition); whatever Zed itself ships for webviews at a newer gpui rev. |

## Feature requests / product decisions

| # | Item | Notes |
|---|------|-------|
| P1 | **Rename app to "Doghouse"** | branding sweep: window title, brand chip, README, cargo bin name?, %APPDATA% dir migration (!), den identity strings |
| P2 | Release versioning for Doghouse + eventually **in-app updates** | needs a release pipeline decision first (GitHub Releases + self-update check?) |
| P3 | Show app version to the LEFT of the "Code Puppy" brand text (top-left toolbar) | DONE: Doghouse (app, HOST_VERSION) chip added left of brand; About panel now labels BOTH versions clearly -- "Doghouse (this app)" vs "code_puppy (agent engine)". Right chip tooltip fixed to name the engine version. |
| P4 | Whistle action uses a horn emoji (postal horn U+1F4EF); want a whistle | Unicode has NO whistle emoji at all. Closest stand-ins: whistling-face vibe (kissing face U+1F617 + dash U+1F4A8), megaphone U+1F4E3, or ship a tiny SVG whistle icon. Decide during P1 branding pass. |
| P5 | Remember window size (and position?) on reopen | DONE: window_rect (x,y,w,h) + window_maximized persisted in session.json; captured each render, change-gated save (rounded to avoid drag jitter), restored at launch with a 480x360 sanity floor. |
