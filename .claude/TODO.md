# TODO — Code Audit Fixes

## Critical

### Security — XSS
- [x] Apply `escapeHtml()`/`escapeAttr()` to all server-sourced data in innerHTML across all components
  - [x] `aeor-toasts.js` — toast message (use textContent instead of innerHTML)
  - [x] `aeor-preview-text.js` — error message in fallback, removed duplicate `_escapeHtml`
  - [x] `aeor-sync.js` — activity feed errors/summaries, form values, table rows, connection options
  - [x] `aeor-settings.js` — client_name, config_dir, data_dir in form values
  - [x] `aeor-conflicts.js` — winner/loser hash, content_type, node_id, conflict_type, path
  - [x] `aeor-dashboard.js` — relationship name and remote_path in sync cards
  - [x] `aeor-connections.js` — connection name/url/auth_type in table rows

### Security — Path Traversal
- [ ] `files.rs:safe_local_path` — replace `contains("..")` with per-segment validation

### Security — Plaintext API Keys
- [ ] Set config file permissions to 0600 on creation
- [ ] (Future) Investigate OS keychain integration

### Performance — Blocking I/O
- [ ] `push.rs` / `pull.rs` — use `tokio::fs` or `spawn_blocking` for filesystem ops

### Performance — Unbounded Memory
- [ ] `remote/mod.rs:download_file` — stream file content instead of buffering
- [ ] `push.rs` — stream upload instead of `std::fs::read` entire file

## Moderate

### Bugs
- [x] `aeor-settings.js` — read input values BEFORE re-render in `_saveSettings`
- [x] `runner.rs` — use configured `sync_interval_seconds` instead of hardcoded 60s
- [ ] `runner.rs` — re-read relationship/connection config each sync cycle (stale data)

### DRY — Frontend
- [ ] Extract `formatSize` — remove duplicates in conflicts, sync, preview-default; import from shared
- [ ] Extract `bindResizeHandle()` utility — used in connections, sync, conflicts, file-browser
- [ ] Extract row-selection/preview-toggle pattern — used in connections, sync, conflicts
- [ ] Extract `_openFolder()` — duplicated in dashboard and settings
- [x] Extract `_escapeHtml` from preview-text — use shared import instead
- [x] Extract `directionLabel()` to shared module
- [x] Extract `directionArrow()` to shared module
- [x] Extract `formatRelativeTime()` to shared module
- [x] Extract `bindResizeHandle()` to shared module
- [x] Extract `openFolder()` to shared module
- [x] Remove dead export `syncBadgeClass()` from shared module

### DRY — Backend
- [ ] Add `NotFound`/`BadRequest` variants to `ClientError` + implement `IntoResponse`
- [ ] Remove 12+ duplicated error-to-status-code string-matching blocks in route handlers
- [x] Extract shared `file_mtime()` from push.rs and pull.rs into sync utility

### Performance — Backend
- [x] Share a single `reqwest::Client` in AppState instead of creating per-request
- [x] Add timeout to `reqwest::Client` (30s)
- [ ] Use `tokio::sync::RwLock` in ConfigStore instead of `std::sync::RwLock`

### Correctness — Frontend
- [ ] Add `response.ok` checks before `.json()` on all fetch calls
- [ ] Toast polling: use `Promise.all()` for parallel fetches, per-relationship timestamps
- [ ] Add `disconnectedCallback` cleanup on components (clear timeouts, remove listeners)
- [ ] Cache version in `aeor-nav.js` — don't refetch on every render

## Minor
- [x] Remove unused `Path` import in `conflicts.rs`
- [ ] Replace magic number `3` (directory) with named constant in shared module
- [ ] Standardize naming: file-browser snake_case → camelCase
- [x] Remove dead export `syncBadgeClass()` from shared module
- [ ] Remove empty constructor in `aeor-nav.js`
- [ ] Fix nested ternary in file-browser relationship selector
- [ ] Validate `pick` field ("winner"/"loser") in conflicts resolve handler
- [x] Guard `event.id[..8]` slice with bounds check in activity.rs
- [ ] Add `console.warn` to empty catch blocks in preview component loader
- [ ] Context menu: check viewport bounds before positioning
- [x] Fix redundant `format!` in `compute_remote_path` (files.rs:216)
- [ ] Allow clearing sync filter via empty string in `UpdateSyncRelationshipRequest`
