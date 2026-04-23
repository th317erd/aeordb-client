# State Database Corruption Report

**From:** AeorDB Client Team
**To:** AeorDB Engine Team
**Date:** 2026-04-23
**Severity:** High — causes data loss and application crashes in production use
**Affected component:** `StorageEngine` append-only log (embedded mode)

---

## Summary

The aeordb-client embeds a `StorageEngine` instance as its local state database (`~/.local/share/aeordb-client/state.aeordb`). This database stores sync metadata, client identity, activity logs, and file sync state. We are experiencing **frequent corruption** of this database, resulting in application startup failures and data loss.

We have hardened the client to recover gracefully from corruption (skip corrupt entries, regenerate identity, auto-backup and recreate as last resort), but the root cause is in the engine and should be addressed there.

---

## Observed Error Messages

All of these have been observed in production logs during normal client operation:

### On database open (entry scanner)
```
WARN aeordb::engine::entry_scanner: Hash verification failed for entry at offset 155542298. Skipping.
WARN aeordb::engine::entry_scanner: Corrupt entry at offset 155547372: Invalid magic bytes. Skipping.
```

### On read after open
```
Error: failed to initialize: server error: failed to read /client/identity.json from state database: Not found: Chunk not found: 5e0e830dd1f53f5ec2f31b7f61d645a8303c3f201db03d5a03b50721c5ebb25c
```

```
Error: failed to initialize: server error: failed to store /sync/activity//.keep in state database: Invalid magic bytes
```

### On write (activity logging)
```
WARN failed to log sync activity for 'E2E Wallpapers': server error: failed to store /sync/activity/7d7136ac.../1776627397867-a3779400.json in state database: Invalid magic bytes
```

### On read after corruption
```
{"error":"server error: failed to list /sync/activity/7d7136ac.../ in state database: Invalid magic bytes"}
```

---

## Reproduction Scenario

The corruption is **reliably reproducible** with the following setup:

1. Start `aeordb-client` with 3 enabled sync relationships (bidirectional)
2. All 3 sync runners start simultaneously in `start_all_enabled()`
3. Each runner performs an initial full sync (push + pull), writing sync metadata to the state DB concurrently
4. After several minutes of operation (concurrent writes from push, pull, activity logging, and periodic sync), the database shows corruption warnings
5. On the next restart, the database fails to read previously-written entries

**Environment:**
- OS: Ubuntu 24.04 (KUbuntu), Linux 6.17
- aeordb version: 0.9.0 (both embedded and server)
- Rust toolchain: stable
- State DB file size when corruption occurs: typically 80-170 MB

---

## Root Cause Analysis

### 1. Concurrent writes without locking

The `StorageEngine` in embedded mode does not appear to synchronize concurrent `store_file` operations. The client spawns 3 sync runner tasks (tokio green threads) that all call `store_file` on the same `StorageEngine` instance simultaneously:

- Runner A writes sync metadata for relationship 1
- Runner B writes sync metadata for relationship 2
- Runner C writes activity log for relationship 3
- The periodic sync timer fires and all 3 runners write again

Each `store_file` appends to the log file. Without a write mutex, two concurrent appends can interleave their bytes, producing entries with:
- Valid header from write A, value bytes from write B
- Truncated entries where write B's header overwrites write A's value
- Hash mismatches (header hash was computed for write A's data, but write B's data landed in that slot)

### 2. Interrupted writes on shutdown

When the client receives a shutdown signal, tokio drops all tasks. If a `store_file` is mid-append (header written but value not yet flushed), the log file contains a partial entry. On next open, the scanner sees:
- Valid magic bytes and header (written)
- Truncated or missing value data (not written)
- Hash verification failure

The scanner correctly skips these, but the data they reference (chunk hashes) becomes orphaned, causing subsequent "Chunk not found" errors when reading files that depended on those entries.

### 3. Chunk reference integrity

Even when the scanner skips a corrupt entry, the higher-level directory/file abstraction can fail because:
- A directory listing references a chunk hash that was stored in the corrupt entry
- `read_file` tries to resolve the chunk hash and gets `Chunk not found`
- This propagates as a fatal error through `DirectoryOps`

---

## Impact on the Client

| Symptom | Frequency | User impact |
|---------|-----------|-------------|
| Startup crash ("failed to read identity") | Every restart after corruption | Application won't start |
| Activity log write failures | During every sync cycle | Missing activity history |
| Browse API errors ("Invalid magic bytes") | On any read of affected entries | File browser shows errors |
| Sync metadata loss | After DB recreation | Full re-sync of all files |
| Identity regeneration | After DB recreation | Client appears as new peer to remotes |

---

## Client-Side Mitigations (Already Implemented)

We've hardened the client to be resilient to corruption:

1. **`open_or_create`** — If the file can't be opened at all, backs it up as `state.aeordb.corrupt.{timestamp}` and creates fresh. The scanner's ability to skip corrupt entries means most corruption is survivable without recreation.

2. **`read_json`** — Returns `None` on read errors or corrupt JSON instead of propagating fatal errors. Callers treat missing data as "not yet created."

3. **`list_directory`** — Returns empty list on errors instead of crashing.

4. **`get_or_create_identity`** — Generates a new identity if the existing one is unreadable, instead of crashing.

5. **`exists`** — Returns `false` on errors (treats corruption as "not found").

These mitigations prevent crashes but do not prevent data loss. The underlying corruption still causes sync metadata, activity logs, and other state to be silently lost.

---

## Recommended Engine Fixes

### Priority 1: Write serialization

Add a `Mutex` or `RwLock` around the append path in `StorageEngine::store`. Multiple callers sharing an `Arc<StorageEngine>` must not interleave their appends. This is the root cause.

```rust
// Current (no synchronization):
pub fn store(&self, key: &[u8], value: &[u8]) -> Result<()> {
    // append to file — not thread-safe
}

// Proposed:
pub fn store(&self, key: &[u8], value: &[u8]) -> Result<()> {
    let _lock = self.write_lock.lock().unwrap();
    // append to file — now serialized
}
```

### Priority 2: Atomic append with fsync

Ensure each entry is fully written and fsynced before returning. A partial write should either:
- Be completed (write + fsync as an atomic unit)
- Be detectable and recoverable (write a "commit marker" after the entry)

### Priority 3: Chunk integrity on read

When `read_file` encounters a "Chunk not found" error, it should return a structured error that callers can handle (e.g., `EngineError::ChunkNotFound`) rather than a generic string error. This would let the client distinguish between "file doesn't exist" and "file exists but is damaged."

### Priority 4: Compaction / GC for corrupt entries

After the scanner skips corrupt entries, those bytes are wasted space. A compaction pass that rewrites the log file, omitting corrupt entries, would reclaim space and eliminate the corrupt regions permanently.

---

## Questions for the Engine Team

1. Is there an existing write synchronization mechanism we're missing? Should we be wrapping `StorageEngine` in our own mutex?
2. Is the `StorageEngine` designed for single-writer use only? If so, we need to serialize all writes through a single channel.
3. Is there a way to run a repair/compaction on a database file that has corrupt entries, preserving the valid data?
4. Would the engine benefit from a WAL (write-ahead log) for crash recovery?

---

## Appendix: Client Architecture

```
aeordb-client process
├── HTTP server (axum) — serves UI and API
├── Sync Runner (tokio tasks)
│   ├── E2E Docs runner → writes to state DB
│   ├── E2E Test runner → writes to state DB
│   └── E2E Wallpapers runner → writes to state DB
├── Activity logger → writes to state DB
└── API handlers → read/write state DB

All share: Arc<StorageEngine> (single state.aeordb file)
```

All writers share a single `Arc<StorageEngine>` instance. There are typically 3-6 concurrent writers (sync runners + API handlers + activity logger).
