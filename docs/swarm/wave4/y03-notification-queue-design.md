# y03 notification queue — design

## Current structure (file:line)

- `crates/flamewm-integrations-linux/src/notifications.rs:24-26`: `MAX_NOTIFICATIONS = 64`,
  `MAX_UI_UPDATES = 256`, and `MAX_TEXT_BYTES = 8192` bound the active toast map, the update
  pipe, and client text.
- `crates/flamewm-integrations-linux/src/notifications.rs:118-144`: `Notification` (view,
  deadline, resident flag) and `NotificationProvider` own the active `BTreeMap<u32, Notification>`,
  the `VecDeque<NotificationUiUpdate>`, and `next_id`. This is the single owner of toast lifetime.
- `crates/flamewm-integrations-linux/src/notifications.rs:152-158`: `earliest_deadline` folds all
  active deadlines into one `Option<Instant>`; an empty map yields `None`, so no timer is armed.
- `crates/flamewm-integrations-linux/src/notifications.rs:331-357` and `:397-400`: `expire` skips
  its scan until `next_deadline` passes. That method is the only deadline source.
- `crates/flamewm-integrations-linux/src/notifications.rs:487-585`: `notify` parses hints, honours
  `replaces_id` (`:524`), rejects a full queue (`:530-536`), allocates an id (`:537-548`), records
  history (`:569-572`), and inserts one `Upsert` (`:573-583`).
- `crates/flamewm-integrations-linux/src/notifications.rs:646-655`: `allocate_id` wraps without
  reusing live ids.
- `crates/flamewm-integrations-linux/src/notification_history.rs:45-54,92-130`: separately bounded
  history (`VecDeque`, entry capacity plus text budget); "every notification is stored" (`:89`).
- `crates/flamewm-shell/src/notification_center.rs:165-214,397-423`: pure typed projection of
  history into the center; it owns no toast and no deadline.
- `crates/flamewm-shell/src/popup_controller.rs:83-149`: popup placement only; never a
  notification-lifetime owner.
- Tests: `src/notifications/tests.rs:351-400` covers deadline coalescing;
  `src/notifications/e2e_tests.rs:337-366` covers `replaces_id` keeping the id and resetting the
  timeout.

## Counted problems

1. No duplicate coalescing. `notify` always allocates a fresh id when `replaces_id == 0`
   (`:537-548`), so repeated identical content stacks up to the 64-toast cap and then returns
   `LimitsExceeded` (`:530-536`), hiding the newest content.
2. Each duplicate consumes two budgets: one active slot plus one `Upsert` (`:582`) and one history
   row (`:571`). The center shows repeated rows and history fills with identical text.
3. Coalescing must not introduce a second timer owner. `next_deadline` must keep reflecting exactly
   one entry per visible toast, and the shell must remain a typed consumer.

## Target design

One mechanism, one owner. `NotificationProvider` in `flamewm-integrations-linux` owns the active
toast map, its single folded deadline, and the update queue. `flamewm-shell` only projects typed
views. An explicit client `replaces_id` stays authoritative. A content-identical repeat that
arrives without a target id reuses the existing toast id, so the same insert path refreshes that
toast's deadline and emits an `Upsert` to the same id. No new bounded slot is consumed, no second
timer owner appears, and no UI or CSS changes. History keeps recording every notification until
step 2, so this first step changes only active-toast behavior.

## Steps (each compiles alone)

1. **Active-queue duplicate coalescing (implemented in this unit).** Add a private typed
   `DuplicateKey` and `duplicate_id` helper in `notifications.rs`. In `notify`, when
   `replaces_id == 0` and the popup decision is `Show`, reuse the id of an active toast with the
   same content instead of allocating a new id; keep the existing full-queue guard, insert, and
   `Upsert` paths. Refresh the deadline in place. Add unit tests for key matching and for the
   `notify` id and queue behavior. Bounds, deadline folding, and history semantics are untouched.
2. **History repeat counts.** Add a typed repeat count to `HistoryEntry` and collapse consecutive
   identical records in `NotificationHistory::push` while keeping `revision` monotonic and the
   text budget correct.
3. **Center projection of repeat counts.** Extend `CenterView` and row nodes in
   `flamewm-shell/src/notification_center.rs` to show the count without changing CSS, geometry, or
   pixel output.
4. **Popup lifetime policy.** Add a typed rule so a coalesced repeat does not raise a second popup
   animation; the shell stays a consumer and gains no lifetime authority.

## Rollback

Step 1 is self-contained: remove `DuplicateKey`, `duplicate_id`, the `coalesce_id` branch in
`notify`, and the two tests from `src/notifications/tests.rs`. There is no persisted format change,
no dependency change, and no UI change, so visuals stay pixel-identical and no stale state can
survive a revert. Steps 2-4 are independent reverts and do not depend on step 1 for compilation.

## Reference-first record

- `.vendor/cosmic-epoch/cosmic-applets/cosmic-applet-notifications/src/subscriptions/freedesktop_proxy.rs:39-50`
  exposes the freedesktop `Notify` contract with `replaces_id: u32`. Choice: keep that explicit
  client id authoritative and implement content coalescing as a second id-reusing path with the
  same lifetime semantics (same id, refreshed timeout, no new history row). No zbus code, layout,
  or dependency is ported; only the id-reuse decision is reused.
- `.vendor/mate-desktop-master/NEWS:25,40` mentions startup-notification support only; this corpus
  has no daemon-side toast queue. No queue algorithm is ported from it, and the gap is recorded
  explicitly rather than filled with guesswork.
