# u36-shell-ui changes

- notification_center: all node ids are now built once (`center_ids()` with `OnceLock`, the same pattern
  `dock.rs` uses for its slot ids) and `Node` borrows them as `&'static str` instead of owning a `String`.
  Composition shift: the centre reprojection formatted about 200 throwaway id strings per pass (5 group
  rows x 3 item rows x 9 children); those allocations and their `String` drops are gone, leaving only the
  clipped text values. The slots are fixed by `MAX_GROUP_SLOTS`/`MAX_ITEM_SLOTS`/`MAX_ITEM_ACTION_SLOTS`,
  so the table is exhaustive and cannot miss a template id.
- notification_center: `nodes_with_focus` preallocates its output vector for the worst case instead of growing
  through repeated reallocations, and `clip` cuts at a `char_indices` byte offset (one copy) rather than
  collecting the first 160 chars into a fresh `String`.
- icon_loader: the pending dedupe probe is now by reference. A repeated request for a still-visible slot,
  which is the common case because views reproject often, no longer builds and then discards an owned key
  tuple (one `String` allocation per repeated request). Dedupe semantics are unchanged: same target, name
  and size.
- icon_loader: `drain_ready` moves the result's own name string into the pending key and back instead of
  cloning it, removing one allocation per delivered raster.
- icon_loader: the dedupe path now increments `coalesced`, the counter the loader already publishes through
  `shell.icon.coalesced` but never changed; the metric reported zero for every coalesced repeat before.
- start: `category_for_action` strips the `start.category.` prefix and compares the suffix against the static
  table instead of formatting nine `start.category.<name>` strings per lookup.
- dock: `running_apps` dedupes with a linear scan over the already-collected apps (a dock holds at most 24)
  instead of building and hashing a `HashSet` on every layout pass, and reserves the result vector.

Review of the recent optimisations in this area found no regression: the dock id table, the centre's
revision gate with borrowed `text_ref` writes, and the budgeted LRU icon cache are all sound and were left
alone. No dead code was removable inside the owned paths: `low_enqueued`/`dropped_low` are still published
from `async_projection/icon_cache.rs` (not an owned path) although no low-priority request path exists, so
they are documented here instead of deleted.

Checks: no `cargo` and no `git` were run (shared checkout, orchestrator runs the batch gate). Compilation
correctness argued by reading: `Node`'s new `&'static str` ids coerce at the only external use
(`main.rs`'s `for node in &nodes` match, `&&str` -> `&str`), and every test that built an owned id was
updated in place.
