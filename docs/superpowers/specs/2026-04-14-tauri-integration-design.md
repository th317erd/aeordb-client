# Tauri v2 Integration Design

**Date:** 2026-04-14
**Status:** Approved

## Overview

Integrate Tauri v2 into the existing aeordb-client binary to provide a native desktop window and systray icon. The webview loads the UI from the client's own HTTP server (`http://127.0.0.1:{port}`). Headless mode skips the Tauri window/tray but Tauri is always compiled in — one binary for all modes.

## Architecture

```
aeordb-client start          → Start HTTP server, open Tauri window + systray
aeordb-client start --headless → Start HTTP server only, no window, no tray
```

**Startup sequence (desktop mode):**
1. Initialize logging
2. Open/create local aeordb state store
3. Start HTTP server (axum) on configured port
4. Launch Tauri app — webview pointed at `http://127.0.0.1:{port}`
5. Show systray icon

**Startup sequence (headless mode):**
1. Initialize logging
2. Open/create local aeordb state store
3. Start HTTP server (axum) on configured port
4. Block on shutdown signal (no Tauri, no tray)

## Systray

**Icon states:**
- Idle (all synced) — default icon
- Syncing (activity) — future: animated icon
- Conflict (needs attention) — future: badge overlay
- Error — future: error icon

**Menu items:**
- "Open AeorDB Client" → show/focus the main window
- Separator
- "Pause All Sync" / "Resume All Sync" → toggle
- Separator
- "Quit" → graceful shutdown

## Window Behavior

- Title: "AeorDB Client"
- Default size: 1024x768
- Minimum size: 800x600
- Close button (X) → hide window, keep running in tray
- Quit from tray menu or CLI `stop` command → graceful shutdown
- Window is resizable and remembers position (Tauri default behavior)

## Tauri Configuration

- `tauri.conf.json` in the `aeordb-client/` binary crate directory
- Webview URL: `http://127.0.0.1:{port}` (resolved at runtime)
- No Tauri bundled assets — all served by our HTTP server
- CSP allows connecting to localhost
- Single window, no multi-window support needed

## Crate Changes

- Add `tauri` v2 dependency to `aeordb-client/Cargo.toml`
- Add `tauri-plugin-shell` for systray interaction
- No feature gates — Tauri always compiled in
- `main.rs` branches on `--headless`: if headless, run HTTP server only; if desktop, run HTTP server + Tauri event loop

## Key Constraint

Tauri v2 owns the main thread event loop on macOS/Linux. The HTTP server must run in a background tokio task, not on the main thread. Startup order:

```rust
// 1. Init state + start HTTP server in background
tokio::spawn(async { axum::serve(...).await });

// 2. Run Tauri on main thread (blocks until quit)
tauri::Builder::default()
  .setup(|app| { /* systray, window config */ })
  .run(context)?;
```

## Out of Scope

- Mobile support (Tauri v2 supports it, but not needed now)
- Auto-updater
- Multiple windows
- Custom title bar / frameless window
