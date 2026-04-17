# Important Details

## Project: aeordb-client
- **Purpose**: Sync-first native desktop client for AeorDB
- **Stack**: Rust + Tauri (backend), Native WebComponents + vanilla JS + HTML5 (frontend) — NO frameworks
- **Architecture**: Single binary (headed/headless), HTTP control API, CLI is a thin HTTP client

## File Locations
- **Config (YAML, human-editable)**: `~/.config/aeordb-client/config.yaml` — connections, relationships
- **Data (aeordb state)**: `~/.local/share/aeordb-client/state.aeordb` — sync state, identity
- **Uses dirs crate for XDG platform paths**

## AeorDB Dependency
- **Git dep**: `aeordb = { git = "ssh://git@github.com/th317erd/aeordb.git", branch = "main" }`
- **Key API**: StorageEngine, DirectoryOps, RequestContext
- **Replication API**: compute_sync_diff, get_needed_chunks, apply_sync_chunks
- **Conflict API**: list_conflicts_typed, resolve_conflict, dismiss_conflict
- **Version API**: file_history, file_restore_from_version

## Architecture (Post-Restructure)
- **Sync uses aeordb native replication** — no custom pull/push/merge
- **Filesystem bridge**: local files ↔ local embedded aeordb (client's responsibility)
- **Replication**: local aeordb ↔ remote aeordb via HTTP (POST /sync/diff + /sync/chunks)
- **Conflicts**: aeordb native /.conflicts/ with winner/loser model (LWW)
- **Direction control**: client-layer (aeordb always bidirectional)
- **Selective sync**: server-side via sync_paths (recently fixed)
- **Delete propagation**: client-layer pre-engine filtering

## Key Modules
- `config.rs` — YAML config store (connections, relationships)
- `state.rs` — Embedded aeordb state store (sync state, identity)
- `sync/replication.rs` — aeordb replication orchestration
- `sync/filesystem_bridge.rs` — local files ↔ local aeordb
- `sync/runner.rs` — continuous sync lifecycle (watcher + SSE + replication)
- `sync/fs_watcher.rs` — filesystem watcher with coalescing
- `sync/sse_listener.rs` — SSE event listener
- `sync/filter.rs` — glob filter matching (for filesystem bridge)
- `sync/hierarchy.rs` — parent/child relationship exclusions
- `sync/content_type.rs` — MIME type detection
- `remote/mod.rs` — HTTP client for remote aeordb
- `server.rs` — axum HTTP server + AppState
- `connections.rs` — connection CRUD (reads from ConfigStore)
- `sync/relationships.rs` — relationship CRUD (reads from ConfigStore)
- `api/routes/conflicts.rs` — proxies aeordb native conflict APIs

## GitHub
- **Repo**: `git@github.com:th317erd/aeordb-client.git`
- **User**: `th317erd`

## Test Count: 58 tests passing
