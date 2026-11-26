# u53-desktop-async-audit — desktop event-path I/O audit

Scope: `crates/flamewm-desktop/src/**`.

## Audit result (what was already off-turn)

Read the current ingress paths before changing anything:

- Wallpaper decode: one bounded worker plus a latest-request mailbox and its own wake fd
  (`wallpaper_loader.rs`). Nothing decodes on the UI turn.
- Icon rasters: `IconService` workers, wake fd sets `icons_pending`, results land only for the
  slot still holding its `IconKey` (`slots.rs`, `projection.rs`).
- Clipboard paste and XDND drops: detached workers (`input.rs`).
- Conversion jobs: detached worker plus a result mailbox drained in `refresh_all`
  (`menu_actions.rs`, `menus.rs`).
- Downloads organizer: inotify watch with a settle poll and a bounded move budget
  (`downloads.rs`); it never scans on pointer motion.
- Launcher metadata: already cached by `len` + `mtime`, so a rescan does not re-parse every
  `.desktop` file (`refresh.rs`).
- Selection, slot, sticky, widget and menu projections: all compare their inputs against the last
  projected snapshot and skip unchanged writes.

So the remaining blocking filesystem work on the event turn was the trash path, and the
remaining repeated filesystem work was one `stat` per launcher per rescan.

## Changes

1. **Trash moves and trash emptying moved off the UI turn** (`files.rs`, `state.rs`,
   `main.rs`, `menu_actions.rs`).
   `TrashJobs` owns a `UnixStream` wake pair and an outcome mailbox; `submit` spawns one
   detached worker per request, the worker performs the freedesktop trash calls, and the reactor
   callback drains outcomes, reports failures and arms one rescan. `confirm.trash` and
   `entry.empty-trash` now only submit and repaint. A failed move still leaves the entry in place
   (never an irreversible delete), and the message is reported from the completion turn.
   Composition shift: a delete of a large directory or an empty-trash on slow/remote storage no
   longer freezes painting and input; the UI cost of the action drops to one spawn plus one
   repaint.

2. **Rescan skips launcher stats the directory watch did not name** (`refresh.rs`, `main.rs`,
   `state.rs`).
   The inotify callback now records the changed names (and an overflow bit) in `ChangedPaths`.
   `refresh_launchers` consumes them: a `.desktop` file a burst never named reuses its parsed
   metadata with no `stat`, a named one is re-stamped and re-parsed only when its stamp moved,
   and an overflow or an empty set (startup, drag, conversion, shortcut) falls back to the old
   re-check-everything behaviour. Composition shift: rescans driven by unrelated desktop churn no
   longer walk the launcher list, which is the expensive case on a network-mounted Desktop folder.

3. **Downloads settle poll allocates nothing for still-arriving files** (`downloads.rs`).
   `tick` no longer clones every tracked name into a temporary `Vec` each poll; it stats in place
   and copies a name only when that entry settles or vanishes. Composition shift: the per-poll
   allocation count drops from one `String` per pending download to zero until a file is ready to
   move.

4. **One theme watch per sync instead of one per projected tile** (`projection.rs`).
   `project_row` no longer calls `ParallelIconService::refresh_theme` for every tile; the single
   call at the top of `sync_document` already runs before `project_slots`, so a new theme
   generation still invalidates every key. Composition shift: removes up to `SLOT_COUNT` throttled
   lock/atomic checks per rescan.

5. **Dead duplicate removed** (`projection.rs`): the second `SLOT_COUNT = 128` now derives from
   `state::SLOT_COUNT`, so the slot budget has one authority and cannot drift.

No behaviour or visual changes: same icons, labels, menus, confirm dialog, and trash semantics.

## Verification

No `cargo`/`git` in this shared checkout (orchestrator builds). Static review plus
`rustfmt --check --edition 2024` on the edited files and the crate root: clean.
