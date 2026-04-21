# TODO — Code Audit Fixes

## Critical

### Security — XSS
- [x] Apply `escapeHtml()`/`escapeAttr()` to all server-sourced data in innerHTML across all components

### Security — Path Traversal
- [x] `files.rs:safe_local_path` — per-segment validation (reject `..` segments)

### Security — Plaintext API Keys
- [x] Set config file permissions to 0600 on creation
- [ ] (Future) Investigate OS keychain integration

### Performance — Blocking I/O
- [x] `pull.rs` — all filesystem ops use `tokio::fs` (create_dir_all, write, remove_file, metadata)
- [x] `push.rs` — directory walk wrapped in `spawn_blocking`, file reads use `tokio::fs`

### Performance — Unbounded Memory
- [x] `remote/mod.rs:download_file` — returns response stream, caller writes chunks to disk
- [x] `push.rs` — streaming upload via `ReaderStream` + `Body::wrap_stream`

## Moderate

### Bugs
- [x] `aeor-settings.js` — read input values BEFORE re-render in `_saveSettings`
- [x] `runner.rs` — use configured `sync_interval_seconds` instead of hardcoded 60s
- [x] `runner.rs` — re-read relationship/connection config each sync cycle

### DRY — Frontend
- [x] Use shared `formatSize` — removed duplicates in conflicts, sync, preview-default
- [x] Use shared `bindResizeHandle()` — replaced in connections, sync, conflicts
- [x] Use shared `openFolder()` — replaced in dashboard, settings
- [x] Use shared `formatRelativeTime()` — replaced in sync
- [x] Use shared `directionLabel()` / `formatUptime()` — replaced in dashboard
- [x] Extract shared utilities to aeor-file-view-shared.js

### DRY — Backend
- [x] `ClientError` variants + `IntoResponse`
- [x] Remove duplicated error-to-status-code string-matching blocks
- [x] Extract shared `file_mtime()` from push.rs and pull.rs

### Performance — Backend
- [x] Share a single `reqwest::Client` in AppState with 30s timeout
- [x] Use `tokio::sync::RwLock` in ConfigStore instead of `std::sync::RwLock`

### Correctness — Frontend
- [x] Add `response.ok` checks before `.json()` on all fetch calls
- [ ] Toast polling: use `Promise.all()` for parallel fetches, per-relationship timestamps
- [ ] Add `disconnectedCallback` cleanup on components (clear timeouts, remove listeners)
- [x] Cache version in `aeor-nav.js`

## Minor
- [x] Remove unused `Path` import in `conflicts.rs`
- [x] Replace magic number `3` (directory) with `ENTRY_TYPE_DIR` constant
- [ ] Standardize naming: file-browser snake_case → camelCase
- [x] Fix nested ternary in file-browser (use `directionArrow()`)
- [x] Validate `pick` field ("winner"/"loser") in conflicts resolve handler
- [x] Guard `event.id[..8]` slice with bounds check in activity.rs
- [x] Add `console.warn` to empty catch blocks in preview component loader
- [ ] Context menu: check viewport bounds before positioning
- [x] Fix redundant `format!` in `compute_remote_path` (files.rs:216)
- [ ] Allow clearing sync filter via empty string in `UpdateSyncRelationshipRequest`
