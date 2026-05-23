# Bug Report — Pull sync silently treats engine permission failures as "nothing to do"

**Filed by:** aeordb-server team (DB team)
**Target:** aeordb-client (`aeordb-client-lib/src/sync/pull.rs` + `api/routes/files.rs` browse proxy)
**Severity:** Medium — defense-in-depth. Engine-side bug just fixed; this report is about hardening the client so a future server-side permission misconfiguration doesn't reproduce the same silent failure.
**Date:** 2026-05-22

---

## Context

Earlier today you filed `bot-docs/bug-reports/2026-05-20-listing-vs-descend-permission-inconsistency.md` (in the aeordb repo) describing a `Family/Susan/` folder that synced as empty even though the user had a direct `crudlify` grant on it.

We tracked the engine-side bug to a trailing-slash mismatch in the permission middleware: `check_direct_permission` walks ancestor levels only and silently skipped the directory's own `.aeordb-permissions` when the URL lacked a trailing slash. That's fixed in aeordb commit `f42778a` — both `permission_middleware.rs:306` and `sync_routes.rs:307` now use `check_path_permission` which falls back to the directory form. Once you pull the new aeordb binary, the Susan folder will sync correctly.

This report is the followup we mentioned. It's about a client-side defense-in-depth issue that's independent of the specific engine bug we just fixed: **any future permission misconfiguration anywhere in the engine produces the same "0 pulled, 0 failed, no error" non-symptom on the client**.

---

## TL;DR

Pull sync's "0 pulled, 0 skipped, 0 failed" outcome is indistinguishable between:

- ✅ Healthy: the engine genuinely has no new files for this relationship.
- ⚠️ Pathological: the engine had files for this relationship but a permission-evaluation bug filtered them out of `/sync/diff` server-side, so the client receives an empty diff.

These are observed differently from the user's perspective:

- Healthy: the local directory is up to date.
- Pathological: the local directory is **empty** even though the in-app file browser shows files on the remote.

But the **client's activity log emits the same line** in both cases: `pull sync complete for 'X': 0 pulled, 0 skipped, 0 failed, 0 deleted, 0 symlinks`. There's no signal in the green-status feed that distinguishes "in sync" from "the engine returned an empty diff that disagrees with what the user can browse."

---

## Why "swallow the 403" isn't quite the right framing

Your original bug report described the client as "silently swallowing the 403 from `/files/browse`." That's not exactly what's happening on the pull path. We dug into it:

- `pull.rs:39 pull_sync` calls `fetch_remote_diff` (POST `/sync/diff`), not `list_directory`.
- The engine's `/sync/diff` does NOT return 403 when permissions deny it. It returns 200 with the denied paths **filtered out** server-side (`sync_routes.rs::is_allowed` retains only paths the user has read access on).
- Pre-fix, `is_allowed` used `check_direct_permission` and dropped Susan's files. The client received a successful, empty-as-far-as-it-knew diff and correctly reported "0 pulled."

So the silent failure mode is **not** "client swallowed a 403." It's "client trusts the engine's filtered diff as ground truth, with no way to detect when the filter is broken."

The browse proxy in `api/routes/files.rs:194-197` DOES surface the 403 — it maps engine errors to `ClientError::BadGateway`, which the user saw in their dev-tools. That's the right behavior; the question is whether the UI presents the BadGateway loudly enough.

---

## What we'd recommend, in priority order

### 1. Per-relationship "expected vs actual" coverage check (high value, ~half-day)

After each pull, compare what the engine reports as the relationship's total file count (via a fresh head listing of `relationship.remote_path/`) against what the client has locally + what the diff just added/modified/deleted.

If there's a meaningful gap — engine says 50 files under this prefix, local has 12, last 30 diffs cumulatively account for 14 — log a **warning** in the activity feed AND surface a toast: "the engine reports more files than your local sync reflects; permissions may have changed."

This catches the entire class of "diff silently incomplete" bugs, not just the specific permission flavor we just fixed.

Suggested log message and warning surface:

```rust
// In pull.rs after the pull loop completes:
if let Ok(head_count) = remote_client.head_recursive_count(&relationship.remote_path).await {
    let local_count = walk_local_count(local_base);
    if head_count.saturating_sub(local_count) > pulled_in_this_run.saturating_add(5) {
        activity_log.warn(
            relationship.id,
            format!(
                "engine reports {} files at {}; local has {} (after pull of {}). \
                 The diff may be omitting paths due to a server-side permission change. \
                 If you expect files to be missing, ignore. Otherwise contact your admin.",
                head_count, relationship.remote_path, local_count, pulled_in_this_run,
            ),
        );
    }
}
```

The +5 slack is to avoid false positives on legitimate small filter-driven differences (glob filters, etc.). Tune as needed.

### 2. Distinguish "empty diff" from "engine returned partial denied result" in the activity log (cheap, ~hour)

Right now `pull.rs:357` always emits `pull sync complete for 'X': N pulled, ...`. Change the activity-log level based on the shape of the response:

| Diff response | Current log | Recommended |
|---|---|---|
| Empty AND no prior changes seen | `info: complete, 0 pulled` | unchanged |
| Empty AND we have a `since_root_hash` that should yield changes | `info: complete, 0 pulled` | `warn: complete, 0 pulled — diff returned empty despite stale since_root_hash` |
| Non-empty | `info: complete, N pulled` | unchanged |
| Engine returned 403/401 on `/sync/diff` itself | `error: sync/diff returned HTTP 403` | unchanged (already handled — keep it loud) |

The middle row is the new behavior. It's a heuristic, but it surfaces the exact failure mode we hit: the local checkpoint thinks something changed, but the diff is empty.

### 3. Browse proxy: ensure the UI handles BadGateway loudly (~hour)

`api/routes/files.rs:197` already maps `list_directory_paginated` errors to `ClientError::BadGateway`, so the API layer is correct. The question is whether the React/Vue/whatever client treats a BadGateway as an empty folder (silent) or as an inline banner (loud).

When the engine returns 403 on a path the user expected to access — common after admin permission edits — the browser should render something like:

> ⚠️ The server denied access to this folder. You may have lost permission since your last visit. Contact your admin if you expect to have access.

Not just an empty list. The empty-list rendering is what gave the user the impression of "the folder is just empty" rather than "I lost permission."

### 4. (Optional, longer-term) Diff signature / freshness check

A persistent worry is that the engine and client get out of sync on what "the user can see." A future hardening would have the diff response include a checksum of the user's grant set at the moment the diff was computed. The client stores it; if the next diff's checksum differs but the diff is empty, that's a strong signal that permissions changed and the client should refresh its local view (and possibly invalidate the local sync state for paths the user can no longer see).

This requires engine cooperation — file a feature request if you decide to go this direction.

---

## Files in your tree we identified during triage

- `aeordb-client-lib/src/sync/pull.rs:357` — the activity log emit point that produces "0 pulled, 0 failed."
- `aeordb-client-lib/src/sync/pull.rs:379 fetch_remote_diff` — the diff fetch; already surfaces non-2xx as errors, no swallow here.
- `aeordb-client-lib/src/remote/mod.rs:142 list_directory` — already surfaces non-2xx as `ClientError::Server`.
- `aeordb-client-lib/src/remote/mod.rs:180 download_file` — already surfaces non-2xx (need to check the streaming-response error path; we didn't verify).
- `aeordb-client-lib/src/api/routes/files.rs:194 browse` — maps engine errors to BadGateway; UI handling is the question.
- `aeordb-client-lib/src/sync/runner.rs:301` — emits "pull for 'X': pulled=N, deleted=N, ..." — same swallow pattern as pull.rs:357.

We didn't find any literal `.unwrap_or(())` or `.ok()` on 403-bearing futures. The "swallow" is structural, not a bug in error handling — it's that the engine and client agree on "empty diff = nothing to do."

---

## What's already fixed on the engine side

For completeness, the upstream fix (aeordb commit `f42778a`):

- `auth/permission_middleware.rs:306` — `check_direct_permission` → `check_path_permission`. The descend path now tries both URL shapes.
- `server/sync_routes.rs:307` — same change for the `/sync/diff` change-filter. Files at directly-shared directories are no longer filtered out of diffs.
- Regression test `sharing_spec.rs::direct_share_on_directory_grants_descend_without_trailing_slash` pins both URL shapes returning 200 for a user with only a direct grant on the path.

Once you pull a binary built from that commit or later, `Family/Susan/` (and any other directly-shared directory) will sync correctly. The client-side hardening above is for the next misconfiguration we haven't found yet.

---

## Contact

DB team can reach back through the usual channel. Happy to assist with the engine-side cooperation if you decide to pursue option 4 (diff signature). For the rest, no engine work needed — entirely client-side.
