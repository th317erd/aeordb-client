# TODO — Restructure for aeordb Native Replication

## Phase 1: Update aeordb dependency + verify APIs compile ✅
## Phase 2: Build the replication module ✅
## Phase 3: Build the filesystem bridge ✅
## Phase 4: Replace conflict system with aeordb native ✅
## Phase 5: Rewire sync runner ✅
## Phase 6: Dead code removal + conflict UI update ✅

## Cleanup
- [ ] Remove redundant client-side path filtering from replication flow (server now handles selective sync)
- [ ] Split config vs state into XDG-correct locations:
  - Config (human-editable): `dirs::config_dir()/aeordb-client/config.yaml` — connections, relationships, settings
  - Data (aeordb state): `dirs::data_dir()/aeordb-client/state.aeordb` — sync state, identity
  - Add `dirs` crate dependency
  - Migrate from single `~/.aeordb-client/state.aeordb` storing everything
  - Dashboard: show both paths, clickable to open native file explorer
  - API endpoint to return paths + open folder action
