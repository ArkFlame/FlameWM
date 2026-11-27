# Unit p55-media-mpris-idle
STATUS: DONE

Crate flamewm-integrations-linux mpris and media_store: with no player present: zero timers, reconnect only on NameOwnerChanged, bounded art cache. Fix deviations.

## Result
STARTED

| file:line | finding | change | risk | status |
|---|---|---|---|---|
| `crates/flamewm-integrations-linux/src/mpris.rs:114-155, 213-271, 307, 374-378` | Absent session bus armed a reactor deadline via `next_retry_in`/`retry_at` and `reconcile()`/`on_ready` called `try_connect` on that deadline, so an idle session with no player woke every 120 s (after seven fast steps) forever. | Removed `ReconnectBackoff`, `retry_at`, `schedule_retry`, `retry_if_due` and their call sites; `next_retry_in` now always returns `None`; `try_connect` runs once at construction; bus loss logs once and stays offline. Player (re)discovery remains only in `apply_owner_changed`, fed by the `NameOwnerChanged` match from `subscribe()`. | A session bus that appears after startup or restarts is no longer auto-discovered until the desktop restarts; snapshots, generation fences and visuals are unchanged. | UNBUILT |
| `crates/flamewm-integrations-linux/src/mpris.rs:895-941, 946-964, 973` (tests) | Backoff/deadline tests pinned the removed timer; the offline child asserted a timer was present. | Removed those four tests, added `absent_session_bus_arms_no_reactor_timer`, updated the offline child assertion to `is_none()`, and removed the deleted fields from the test provider. | Test-only; no runtime behavior change. | UNBUILT |
| `crates/flamewm-integrations-linux/src/media_store.rs:17-18, 121-151, 315-328, 340-353` (audit) | No in-memory art cache exists in `mpris.rs` or `media_store.rs`; media imports are capped at 64 MiB, decoded and deduplicated by hash-named managed files, so the bounded-art-cache property already holds. | None (audit only). | None. | UNBUILT (no edit) |
| `crates/flamewm-integrations-linux/src/lib.rs:173-178` and `crates/flamewm-wm-x11/src/runtime/mod.rs:616-619` (audit) | The runtime still calls `next_retry_in` each turn, but it now returns `None`, so no reactor wake is armed by MPRIS. | None (out-of-scope files untouched). | None. | UNBUILT (no edit) |

Reference-first: `.vendor/LingmoOS-main/lib/libsys/mpris/mprismanager.cpp:53-80` connects once, logs a failed connection without retrying, subscribes `NameOwnerChanged` and does a single registered-name scan at startup; no timer or polling. This matches the new FlameWM behavior. `.vendor/kwin-master` and `.vendor/smithay-master` had no closer MPRIS idle precedent; no clone was needed. No build, tests, git or Python were run in this shared checkout.

PASS — with no player present MPRIS now arms zero timers: the only reactor deadline was removed and `next_retry_in` always returns `None`. Player discovery and reconnection are driven solely by the `NameOwnerChanged` match, and the bounded art/media-store property needed no change because no art cache exists and media imports are size-capped and hash-deduplicated. The change is confined to one production file plus its tests and this record; visuals and generations are unchanged. Every edit is UNBUILT and awaits the orchestrator's build, tests and commit.
