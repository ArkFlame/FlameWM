# u22 desktop/wm second-pass log

Second-pass scout of `crates/flamewm-desktop/src/**`. Earlier units already made the per-turn
refresh flag-gated, removed the duplicate refresh on resize/visibility, and cached slot inputs.
The remaining composition share in this area is (a) the same `refresh_all` running for every
projection wake even while the whole desktop is hidden, (b) the event callback's pre-refresh
running on pointer motion, (c) full-set document rewrites for selection chrome and unchanged
tiles, and (d) `.desktop` re-parsing on every directory rescan.

- main/flush: a fully obscured desktop now defers `refresh_all` entirely and leaves every dirty
  flag armed for the uncover flush; only completed icon results are still applied so the bounded
  icon result cache cannot back up while covered. `widgets::is_obscured` exposes the existing
  widget visibility gate. Change: while FullyObscured, settings/workspace/weather/media/inotify
  wakes no longer run rescan + full document projection behind other windows; the uncover turn
  (Visibility resume + same-turn flush) applies the accumulated state. Composition shift: the
  projection share of every steady-state wake drops to zero while the desktop is covered.
- main/event: the pre-handler `refresh_all` is now skipped for Motion actions (rubber selection,
  item and widget drags) and for external XDND `Drag` events, matching the existing hover skip.
  Those handlers only stage pending state for the flush turn and the previous flush already left
  the document current. Composition shift: one full refresh pass disappears per pointer-motion
  and drag event at input rate.
- slots: an unchanged tile no longer formats its node id and re-asserts `visible` on every sync
  unless a drag ghost is active and can have hidden a source tile. Composition shift: the common
  sync issues no per-tile document writes and no id allocation (was one `String` and one
  `set_visible` lookup per visible tile per sync).
- rubber: `sync_selection_visuals` keeps the last projected (fill alpha, accent, selected set)
  and returns without writing or allocating when they are unchanged; otherwise it writes only
  tiles whose membership changed, plus every selected tile when the style changed. Composition
  shift: steady-state syncs drop `2N` color setters and `N` id formats; selection changes stay
  O(changed).
- refresh: launcher metadata is reused after a rescan when the `.desktop` file's length and
  modification time are unchanged, instead of clearing the map and re-reading/re-parsing every
  launcher. Unreadable files are never reused, so a transient stat failure retries next turn.
  Composition shift: launcher reads (open/read/parse per file) leave the UI thread for
  rescan bursts caused by unrelated desktop changes.
