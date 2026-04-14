# TODO — Tauri v2 Integration

## Phase 1: Tauri scaffolding
- [ ] Add tauri, tauri-build deps to aeordb-client/Cargo.toml
- [ ] Create aeordb-client/build.rs
- [ ] Create aeordb-client/tauri.conf.json
- [ ] Create placeholder icon
- [ ] Verify: cargo build + cargo test pass

## Phase 2: Main thread restructure
- [ ] Rewrite main.rs — non-async main(), manual tokio runtime
- [ ] HTTP server on background thread, Tauri on main thread
- [ ] Server readiness signal before Tauri window opens
- [ ] Headless mode bypasses Tauri
- [ ] CLI subcommands still work
- [ ] Test gate: 67 tests pass

## Phase 3: Systray
- [ ] Systray icon with menu
- [ ] Open / Pause / Resume / Quit actions
- [ ] Quit triggers graceful shutdown

## Phase 4: Window behavior
- [ ] Close-to-tray (X hides, not quits)
- [ ] Window title, sizing (1024x768 default, 800x600 min)
- [ ] Re-show from tray

## Phase 5: Graceful shutdown wiring
- [ ] All shutdown paths work (CTRL+C, SIGTERM, API, tray, CLI)
- [ ] All 67 tests still pass
