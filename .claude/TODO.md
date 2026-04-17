# TODO — Restructure for aeordb Native Replication

## Phase 1: Update aeordb dependency + verify APIs compile
- [ ] Update aeordb git dependency
- [ ] Write minimal test calling compute_sync_diff()
- [ ] Verify all new APIs are accessible and compile

## Phase 2: Build the replication module
- [ ] Create sync/replication.rs
- [ ] Orchestrate local (library) ↔ remote (HTTP) sync
- [ ] compute_sync_diff() locally, POST /sync/diff remotely
- [ ] Chunk exchange in both directions
- [ ] Replace sync/engine.rs, sync/push.rs, sync/reconcile.rs

## Phase 3: Build the filesystem bridge
- [ ] Create sync/filesystem_bridge.rs
- [ ] Ingest: fs watcher → DirectoryOps::store_file() into local aeordb
- [ ] Project: after replication, read changed files → write to filesystem
- [ ] Write-back suppression (ignore watcher events from our own writes)

## Phase 4: Replace conflict system
- [ ] Delete sync/conflicts.rs (our custom conflict tracking)
- [ ] Conflict UI calls list_conflicts_typed() on local aeordb
- [ ] Resolution calls resolve_conflict() / dismiss_conflict()
- [ ] Update API routes to proxy these calls

## Phase 5: Update the sync runner
- [ ] sync/runner.rs orchestrates: filesystem bridge + periodic replication
- [ ] SSE listener triggers replication cycle
- [ ] Remove dead code: remote/upload.rs, old engine/push/reconcile

## Phase 6: Update UI + tests
- [ ] Conflict UI uses aeordb ConflictRecord format (winner/loser)
- [ ] Remove "Keep Both" option
- [ ] Update/rewrite tests for new sync flow
- [ ] Remove dead code (old engine.rs, push.rs, reconcile.rs, upload.rs, conflicts.rs)
- [ ] All tests pass

## Cleanup
- [ ] Remove redundant client-side path filtering from replication flow (server now handles selective sync)
