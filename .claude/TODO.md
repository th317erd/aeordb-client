# TODO — File Browser Phase 3

## Track A: Infinite scroll with pagination
- [ ] Update browse API to pass limit/offset to remote and return pagination metadata
- [ ] Update file browser UI to fetch pages on scroll
- [ ] Virtual DOM recycling (only render visible rows/cards)
- [ ] Loading indicator at bottom while fetching next page

## Track B: File interactions
- [ ] File preview panel (click file → show preview)
  - Text: rendered content
  - Images: <img> from /api/v1/files/{id}/{path}
  - Video/audio: <video>/<audio> tags
  - Binary: metadata only
- [ ] "Open Locally" button → POST /api/v1/files/{id}/open
- [ ] Upload button → file picker → PUT /api/v1/files/{id}/{path}
- [ ] Delete with confirmation → DELETE /api/v1/files/{id}/{path}
- [ ] Rename → prompt for new name → remote_client.rename_file()
- [ ] Context menu (right-click: open, preview, rename, delete, download)
