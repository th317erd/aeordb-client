# Important Details

## Project: aeordb-client
- **Purpose**: Sync-first native desktop client for AeorDB
- **Stack**: Rust + Tauri (backend), Native WebComponents + vanilla JS + HTML5 (frontend) — NO frameworks
- **Architecture**: Single binary (headed/headless), HTTP control API, CLI is a thin HTTP client
- **AeorDB docs location**: `../aeordb/docs/`

## AeorDB Dependency
- **Crate name**: `aeordb` (from `aeordb-lib/` directory in the aeordb repo)
- **Git dep**: `aeordb = { git = "ssh://git@github.com/th317erd/aeordb.git", branch = "main" }`
- **Repo remote**: `git@github.com:th317erd/aeordb.git`
- **Branch**: `main`
- **Local path**: `/home/wyatt/Projects/aeordb-workspace/aeordb/aeordb-lib/`
- **Key API**: StorageEngine::open/create, DirectoryOps::new(&engine), RequestContext::system()
- **File ops**: store_file(&ctx, path, data, content_type), read_file(path), delete_file(&ctx, path), list_directory(path), exists(path), get_metadata(path)

## Implementation Progress
- **Step 1**: Skeleton — Cargo workspace, axum HTTP server, /api/v1/status ✅
- **Step 2**: Local state store — embedded aeordb, client identity ✅
- **Step 3**: Connection management — CRUD + connectivity test ✅
- **Step 4**: Sync relationship CRUD ✅
- **Step 5**: Pull sync engine — one-shot recursive directory sync ✅
- **Step 6**: CLI control mode — full subcommand structure ✅
- **Step 7**: SSE listener infrastructure ✅
- **Step 8**: Push sync engine + filesystem watcher infrastructure ✅
- **Step 9**: Conflict detection and management ✅
- **Step 10**: Glob filters for sync relationships ✅
- **Step 11**: Multi-relationship hierarchy awareness ✅
- **Step 12**: Offline reconciliation engine ✅
- **Step 13**: Symlink support — DEFERRED (requires aeordb engine changes)
- **Steps 14-15**: WebComponent UI shell + management pages ✅
- **Step 16**: Tauri webview + systray — TODO
- **Step 17**: Auth infrastructure ✅
- **Step 18**: Resilience (retry, graceful restart, daemon) — TODO

## Test Count: 67 tests passing

## Key Modules
- `aeordb-client-lib/src/state.rs` — Embedded aeordb state store
- `aeordb-client-lib/src/connections.rs` — Remote connection management
- `aeordb-client-lib/src/remote.rs` — HTTP client for remote aeordb
- `aeordb-client-lib/src/sync/engine.rs` — Pull sync engine
- `aeordb-client-lib/src/sync/push.rs` — Push sync engine
- `aeordb-client-lib/src/sync/conflicts.rs` — Conflict management
- `aeordb-client-lib/src/sync/filter.rs` — Glob filter matching
- `aeordb-client-lib/src/sync/fs_watcher.rs` — Filesystem watcher with coalescing
- `aeordb-client-lib/src/sync/sse_listener.rs` — SSE event listener
- `aeordb-client-lib/src/server.rs` — HTTP server (axum)
- `aeordb-client/src/main.rs` — Binary entry point, clap CLI
- `aeordb-client/src/cli/` — CLI subcommand handlers

## GitHub
- **Repo**: `git@github.com:th317erd/aeordb-client.git`
- **User**: `th317erd`

## User Preferences
- HATES React/Vue/Angular — strictly native WebComponents, vanilla JS, HTML5
- No frameworks, no bundlers, no nonsense
- Tokio + axum confirmed as preferred async runtime and HTTP server
