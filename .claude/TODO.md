# TODO — File Browser

## Phase 1: Backend — file serving + browse API
- [ ] Create api/routes/files.rs
- [ ] GET /browse/{relationship_id}/{*path} — directory listing with sync annotations
- [ ] GET /files/{relationship_id}/{*path} — smart file serving (local if synced, remote fallback)
- [ ] GET /files/{relationship_id}/{*path}?source=remote — force remote
- [ ] PUT /files/{relationship_id}/{*path} — upload (proxy to remote)
- [ ] DELETE /files/{relationship_id}/{*path} — delete (proxy to remote)
- [ ] POST /files/{relationship_id}/open — open::that() on local file
- [ ] Path traversal security (canonicalize, validate prefix)
- [ ] Streaming response for large files
- [ ] Tests: browse, serve local/remote, path traversal rejection, upload/delete proxy

## Phase 2: UI — navigation + directory listing + tabs
- [ ] Add File Browser to aeor-nav.js
- [ ] Route in aeor-app.js
- [ ] aeor-file-browser.js — main component
- [ ] Relationship selector (top-level "folders" with direction arrows)
- [ ] Tab bar (open multiple folders, +/close/switch)
- [ ] Directory listing table (name, size, type, date, sync status)
- [ ] Breadcrumb navigation
- [ ] Sync status badges

## Phase 3: UI — interactions
- [ ] File preview panel (text, images, video/audio, binary metadata)
- [ ] "Open Locally" button (open::that on synced file)
- [ ] Upload button + file picker
- [ ] Delete with confirmation
- [ ] Context menu (right-click: open, preview, delete, download)
- [ ] Rename/move — PLACEHOLDER (waiting on aeordb API)

## Phase 4: CSS + polish
- [ ] File type icons
- [ ] Relationship direction arrows
- [ ] Loading/error states
