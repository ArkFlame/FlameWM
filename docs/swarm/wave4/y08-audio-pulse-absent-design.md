# y08 — PulseAudio absence: bounded retry, an explicit record, graceful volume degradation

## Current structure (file:line)

Line numbers are after step 1 landed.

- `crates/flamewm-audio-helper/src/main.rs:47-63` — helper entry. `PulseAdapter::connect`
  failure publishes an `Unavailable` snapshot and exits `ExitCode::FAILURE`; a successful
  `connect` enters the request loop even when no Pulse server is present
  (`crates/flamewm-integrations-linux/src/pulse.rs:110-134`).
- `crates/flamewm-audio-helper/src/session.rs:15-19` — `READY_WAIT = 2s`,
  `READY_POLL_START = 25ms`, `READY_POLL_MAX = 500ms`.
- `crates/flamewm-audio-helper/src/session.rs:29-43` — `wait_until_ready`, exponential probe
  backoff capped at 500ms and bounded by the deadline.
- `crates/flamewm-audio-helper/src/session.rs:49-94` — `serve`; the first request waits for
  readiness, then every path publishes only for `get` or a rejected write. Step 1 added the
  explicit absence record at `:62-67`.
- `crates/flamewm-audio-helper/src/session.rs:96-98` — `reconcile`, the rejected-write path.
- `crates/flamewm-audio-helper/src/session.rs:274-296,298-316` — the rejection test and the new
  `absent_server_is_recorded_without_a_get_or_a_write` test.
- `crates/flamewm-integrations-linux/src/audio_helper.rs:26-37` — WM-side `schedule_retry`,
  300ms doubling to 8s; `:151-219` writer/deploy with backoff gate; `:276-328` reader reaps a
  crashed helper and re-arms backoff; `:354-368` `publish_unavailable` keeps the last
  generations so repeated failures dedupe.
- `crates/flamewm-integrations-core/src/pulse.rs:9-16` — `PulseState` (`Unavailable`,
  `Connecting`, `Ready`, `Failed`); `:42-57` `BackoffConfig` (300ms/8s/8 attempts);
  `:159-167` `next_reconnect_delay_ms`; `:169-190` `snapshot`.
- `crates/flamewm-integrations-linux/src/pulse.rs:599-655` — `retry_connection` and
  `mark_disconnected`; `:657-664` timer restart, with a 5s keep-alive after backoff is spent
  (`:22-26`).
- `crates/flamewm-shell-core/src/lib.rs:86-118` — `StatusViews` degrades audio from
  `ServiceAvailability::Unavailable`; `:195-203` `StatusPopovers` builds the audio popup.

## Counted problems

1. **One missing absence record.** Before step 1, `serve` (`session.rs:62-64`) only logged a
   failed first ready wait. When the first request is not `get` and no write is rejected, no
   snapshot is published, so the shell keeps its last audio state instead of degrading.
   Count: 1 publish site with no record.
2. **One duplicate record after step 1.** A failed first wait followed by a rejected write
   publishes the same `Unavailable` snapshot at `session.rs:66` and again at `:96-98`. Nothing
   dedupes at the helper boundary; the host dedupes by generation, not by content.
   Count: 1 repeated publication.
3. **One dead terminal state.** `PulseState::Failed` (`integrations-core/src/pulse.rs:15`) is
   never assigned. `mark_disconnected` (`integrations-linux/src/pulse.rs:636-655`) resets to
   `Unavailable` and retries forever on the 5s keep-alive, so an exhausted backoff leaves no
   record that the attempt window was consumed. Count: 1 unused variant.
4. **One unguarded widget field.** `StatusPopovers::from_snapshot` uses raw
   `system.audio.muted` (`shell-core/src/lib.rs:202`) while `StatusViews` uses the guarded
   `audio_muted` (`:88-89`). An `Unavailable` snapshot can therefore still read as muted in
   the audio popup. Count: 1 field.

## Reference precedent (path + choice)

- `.vendor/mate-desktop-master/libmate-desktop/mate-desktop-thumbnail.c:258-275` —
  `mate_desktop_thumbnail_factory_is_disabled` turns a capability flag into a typed predicate;
  consumers short-circuit on it instead of assuming the service exists. The GSettings callback
  at `:504-577` writes that flag once and clears the detail list, so the disabled state is a
  recorded fact, not a repeated probe.
- `.vendor/mate-desktop-master/libmate-desktop/mate-bg.c:360-365` — an explicitly missing
  resource falls back to a default rather than aborting wallpaper setup.
- Choice: port the *pattern* only — record absence as typed state, then let consumers degrade
  through a predicate — into `AudioSnapshot`'s existing `ServiceAvailability` field and the
  existing snapshot publish path. No vendor file, symbol, or dependency is copied. The corpus
  contains no PulseAudio implementation, so there is no vendor audio code to transplant; this
  uncertainty is recorded rather than filled with an invented upstream behavior. No new deps,
  no wire-format change, normal-state visuals unchanged.

## Target design

- Absence is a first-class snapshot, published at the moment the bounded ready wait fails and
  before any request is applied. The helper does not abort: it keeps serving and idles out.
- Retry remains bounded and doubling at both layers: readiness probes 25ms→500ms within 2s
  (`session.rs:29-43`) and WM respawns 300ms→8s within 8 attempts, then a 5s keep-alive
  (`audio_helper.rs:26-37`, `pulse.rs:657-664`).
- The shell degrades the volume view from the `Unavailable` snapshot; no widget is removed and
  the popup's muted flag follows the same guard as the status icon.
- Backoff exhaustion is recorded in `PulseController` as `PulseState::Failed` and exposed by a
  typed accessor, while the slow probe continues.
- Consecutive identical snapshots are deduped at one boundary so absence is emitted once.

## Steps (each compiles alone)

1. **Implemented here (UNBUILT):** publish `backend.snapshot()` when the first ready wait fails
   in `session.rs::serve`, and add `absent_server_is_recorded_without_a_get_or_a_write` plus the
   updated rejection assertion. No wire or type change, no new dependency; compiles alone.
2. **Planned:** add a small publish-state wrapper at `main.rs:40-45` that skips a snapshot equal
   to the last one published, so the record from step 1 is emitted once. Compiles alone.
3. **Planned:** add `PulseController::record_failed` and `state()`, and call them from
   `mark_disconnected` when `next_reconnect_delay_ms` returns `None`; keep the slow timer.
   Compiles alone.
4. **Planned:** build `StatusPopovers`' `muted` from the same guarded value as `StatusViews`
   (`shell-core/src/lib.rs:202`); the visual delta is confined to the degraded state.
   Compiles alone.

## Rollback

- Step 1: remove the two comment lines and `publish(&backend.snapshot())` at
  `session.rs:62-67`, restore the single-line assertion in
  `rejected_write_publishes_the_true_snapshot`, and delete
  `absent_server_is_recorded_without_a_get_or_a_write`. No state or data migration, no runtime
  identity change.
- Step 2: drop the publish-state wrapper and restore the direct `publish` call.
- Step 3: drop `record_failed`/`state` and restore `mark_disconnected`'s current body.
- Step 4: restore `muted: system.audio.muted`.
- Every step keeps normal-state visuals pixel-identical and adds no dependencies.
