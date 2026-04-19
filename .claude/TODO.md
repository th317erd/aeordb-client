# TODO — Preview Components + Sync Dot Fix

## Phase 1: Preview loader + fallback + default preview
- [ ] Create previews/ directory under static/components/
- [ ] Build preview loader with three-tier fallback (exact → group → default)
- [ ] aeor-preview-default.js — binary metadata preview (size, type, versions, hash)
- [ ] Refactor file browser to use dynamic preview loading
- [ ] Remove old inline _renderPreviewPanel logic

## Phase 2: Group preview components
- [ ] aeor-preview-image.js — <img> for all standard image types
- [ ] aeor-preview-text.js — <pre><code> for plain text, code files
- [ ] aeor-preview-video.js — <video> player
- [ ] aeor-preview-audio.js — <audio> player

## Phase 3: Text subtypes
- [ ] Markdown rendering in aeor-preview-text.js (detect .md, render HTML)
- [ ] Code block styling for source files

## Fix: Sync dots
- [ ] Green dot = file exists locally (use has_local, not sync_status)
- [ ] No dot / hidden = not local
- [ ] "Download" only shows when NOT local
- [ ] "Open Locally" only shows when local
