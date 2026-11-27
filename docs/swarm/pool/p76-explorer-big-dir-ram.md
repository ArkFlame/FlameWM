# Unit p76-explorer-big-dir-ram
STATUS: DONE

Crate flamewm-explorer: directory with 100k entries: lazy/virtualised list, bounded thumbnail cache bytes, scan off the UI thread, cancel on navigate. Fix deviations, max 3 edits.

## Result

- `crates/flamewm-explorer/src/lib.rs:185-189` — finding: `Tab` derived `Clone` copied the whole `entries` Vec; opening a folder from a 100k-row listing cloned 100k `Entry` values on the UI thread before `visit`/`reset` cleared them, and the clone was held in `pending_navigation` until the scan landed. change: added `Tab::clone_for_navigation()`, carrying only `history` and `cursor` with an empty listing. **UNBUILT**. risk: the candidate `revision` now starts at 0 instead of inherited+1; only monotonic identity uses it, so row labels, thumbnail pass and pixels are unchanged.
- `crates/flamewm-explorer/src/app/navigation.rs:215,243` — finding: both navigation paths (open path, back/forward) called `Tab::clone()`, duplicating the current listing. change: switched both call sites to `clone_for_navigation()`. **UNBUILT**. risk: the full navigation history is still cloned; search/selection/scroll state was already cleared by `visit`/`back`/`forward`, so behavior and visuals stay identical.
- `crates/flamewm-explorer/src/lib.rs:102-138`, `crates/flamewm-explorer/src/workers.rs:123-161`, `crates/flamewm-explorer/src/thumbs.rs:34-36,669-696`, `crates/flamewm-explorer/src/app/projection.rs:656-723` — finding: no deviation: readdir and sort run on the dedicated scan thread with a generation/cancel check every 64 entries, streamed batches are bounded, the LRU enforces 256 KiB of pixel bytes, and a full projection copies at most `ROW_SLOTS` (24) rows. change: none. risk: none.
- Evidence: `.vendor/cosmic-epoch/cosmic-files/src/tab.rs:1020-1134` rebuilds items from a location instead of copying a tab item vector on navigation; `.vendor/LingmoOS-main/fm/thumbnailer/thumbnailcache.cpp` uses a disk-keyed cache with no byte-bound precedent. No `cargo` build or test was run (brief forbids it).

Every production edit above is UNBUILT; the orchestrator builds and tests after the batch lands.

PASS — the four named behaviors were already present and were confirmed; the one remaining large-folder RAM deviation was the UI-thread `Tab::clone()` on navigate, which duplicated a 100k-row listing just before it was discarded. It is replaced by a history-only navigation candidate at both call sites, so scanning still runs off the UI thread with generation-based cancellation, thumbnails stay byte-bounded, and rendering remains pixel-identical.
