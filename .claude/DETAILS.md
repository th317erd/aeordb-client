# Important Details

## Project: aeordb-client
- **Purpose**: Sync-first native desktop client for AeorDB
- **Stack**: Rust + Tauri (backend), Native WebComponents + vanilla JS + HTML5 (frontend) — NO frameworks
- **Architecture**: Single binary (headed/headless), HTTP control API, CLI is a thin HTTP client

## File Locations
- **Config (YAML, human-editable)**: `dirs::config_dir()/aeordb-client/config.yaml` — connections, relationships
- **Data (aeordb metadata)**: `dirs::data_dir()/aeordb-client/state.aeordb` — sync metadata, identity
- **NO file content stored locally in aeordb** — files live on filesystem, metadata only in aeordb

## Sync Architecture
- **Push**: scan filesystem → hash → compare metadata → PUT /engine/{path} to remote
- **Pull**: POST /sync/diff to remote → GET /engine/{path} → write to filesystem
- **Local aeordb**: metadata only (FileSyncMeta, SyncCheckpoint, identity)
- **No double storage**: files exist once (on filesystem), not duplicated in aeordb
- **Direction control**: client-layer (pull_only, push_only, bidirectional)
- **Conflicts**: aeordb native /.conflicts/ with winner/loser model (LWW)
- **Selective sync**: server-side via sync_paths

## Key Modules
- `config.rs` — YAML config store (connections, relationships)
- `state.rs` — Embedded aeordb (sync metadata, identity only)
- `sync/push.rs` — filesystem → remote (hash comparison, mtime fast-skip)
- `sync/pull.rs` — remote → filesystem (POST /sync/diff + GET /engine/{path})
- `sync/replication.rs` — thin orchestrator (calls push + pull based on direction)
- `sync/metadata.rs` — FileSyncMeta, SyncCheckpoint, SyncMetadataStore
- `sync/runner.rs` — continuous sync lifecycle (watcher + SSE + periodic)
- `sync/fs_watcher.rs` — filesystem watcher with coalescing
- `sync/sse_listener.rs` — SSE event listener for remote changes
- `sync/filter.rs` — glob filter matching
- `sync/hierarchy.rs` — parent/child relationship exclusions
- `sync/content_type.rs` — MIME type detection
- `remote/mod.rs` — HTTP client for remote aeordb
- `server.rs` — axum HTTP server + AppState
- `connections.rs` — connection CRUD (ConfigStore)
- `sync/relationships.rs` — relationship CRUD (ConfigStore)

## Test Count: 110 tests passing

## GitHub
- **Repo**: `git@github.com:th317erd/aeordb-client.git`
- **User**: `th317erd`
