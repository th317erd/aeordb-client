# TODO — Code Audit Fixes

## Critical — All Done

- [x] XSS sweep (escapeHtml/escapeAttr across all components)
- [x] Path traversal hardening (per-segment validation)
- [x] Config file permissions (0600)
- [x] Async I/O (tokio::fs for all sync filesystem ops)
- [x] Streaming transfers (download chunks to disk, upload via ReaderStream)

## Moderate — All Done

- [x] Settings save bug (read inputs before re-render)
- [x] Sync interval setting wired into runner
- [x] Stale config refresh in sync loop
- [x] Frontend DRY (shared formatSize, bindResizeHandle, openFolder, etc.)
- [x] Backend DRY (ClientError + IntoResponse, shared file_mtime)
- [x] Shared reqwest::Client with 30s timeout
- [x] tokio::sync::RwLock for ConfigStore
- [x] response.ok checks on all fetch calls
- [x] SSE for toast notifications (replaced polling)
- [x] Toast debounce (2s window, grouped summaries)
- [x] Version caching in nav
- [x] disconnectedCallback cleanup on dashboard, settings, nav

## Minor — All Done

- [x] Unused Path import removed
- [x] ENTRY_TYPE_DIR constant replacing magic number 3
- [x] Nested ternary replaced with directionArrow()
- [x] pick field validation in conflicts resolve
- [x] event.id slice bounds check
- [x] console.warn in preview loader catch blocks
- [x] Context menu viewport bounds check
- [x] Redundant format! fixed
- [x] Filter clearing via empty string

## Remaining (deferred)

- [ ] Standardize naming: file-browser snake_case → camelCase (cosmetic, low priority)
- [ ] (Future) OS keychain integration for API keys
