# Important Details

## Project: aeordb-client
- **Purpose**: Native desktop client for AeorDB
- **Stack**: Rust + Tauri (backend), Native WebComponents + vanilla JS + HTML5 (frontend) — NO frameworks
- **AeorDB docs location**: `../aeordb/docs/`

## AeorDB Summary
- Content-addressed file database (BLAKE3 hashing), append-only WAL
- Filesystem-like data model: files at paths (e.g., `/users/alice.json`)
- Git-like versioning: snapshots, forks, diff/patch, export/import
- NVT-based indexing: opt-in per-directory, supports u64, i64, f64, string, timestamp, trigram, phonetic
- JSON query API: boolean logic (and/or/not), comparison operators, sorting, pagination, projections, aggregations
- WASM plugin system: parser plugins (transform non-JSON to queryable JSON), query plugins (custom data-layer logic)
- Built-in HTTP API (axum-based), default port 3000
- Auth: self-contained JWT, API keys, user/group management, path-level permissions
- Single binary, zero external dependencies

## Key API Endpoints
- `PUT/GET/DELETE/HEAD /engine/{path}` — File CRUD
- `POST /query` — Indexed queries
- `POST /version/snapshot`, `GET /version/snapshots` — Snapshots
- `POST /version/fork`, `GET /version/forks` — Forks
- `GET/POST /upload/check`, `PUT /upload/chunks/{hash}`, `POST /upload/commit` — Upload protocol
- `POST /auth/token`, `POST /auth/refresh` — Auth
- `POST /admin/gc`, `POST /admin/tasks/*` — Admin operations
- `GET /events/stream` — SSE event stream
- `GET /admin/health`, `GET /admin/metrics` — Monitoring

## User Preferences
- HATES React/Vue/Angular — strictly native WebComponents, vanilla JS, HTML5
- No frameworks, no bundlers, no nonsense
