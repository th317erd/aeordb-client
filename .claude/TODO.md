# TODO — Local Storage Rework (Metadata-Only)

## Phase 1: Sync metadata module
- [ ] Create sync/metadata.rs — FileSyncMeta + SyncCheckpoint stored in local aeordb
- [ ] CRUD methods for per-file metadata and per-relationship checkpoints
- [ ] Tests for metadata storage

## Phase 2: Push sync (filesystem → remote)
- [ ] Create new sync/push.rs — scan filesystem, hash, compare metadata, upload changed
- [ ] Use mtime fast-skip before hashing
- [ ] Simple PUT /engine/{path} for upload (no chunking for now)
- [ ] Handle symlinks (POST /engine-symlink/{path})
- [ ] Handle deletes (DELETE /engine/{path} if propagation enabled)
- [ ] Update local metadata after successful push
- [ ] Tests against mock server

## Phase 3: Pull sync (remote → filesystem)
- [ ] Create new sync/pull.rs — POST /sync/diff + GET /engine/{path}
- [ ] Download changed files directly to filesystem
- [ ] Handle symlinks, deletions
- [ ] Store new remote root hash as checkpoint
- [ ] Update per-file metadata
- [ ] Tests against mock server

## Phase 4: Rework replication module
- [ ] sync/replication.rs becomes thin orchestrator: push + pull
- [ ] No more library-level compute_sync_diff/apply_sync_chunks calls
- [ ] HTTP-only interaction with remote

## Phase 5: Update runner
- [ ] Filesystem watcher triggers push
- [ ] SSE listener triggers pull
- [ ] Periodic safety net triggers both
- [ ] Delete sync/filesystem_bridge.rs

## Phase 6: Clean up + tests
- [ ] Remove unused imports (compute_sync_diff, apply_sync_chunks, get_needed_chunks)
- [ ] Update DETAILS.md
- [ ] All tests pass
