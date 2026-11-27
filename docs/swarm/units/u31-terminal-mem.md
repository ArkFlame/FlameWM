# u31 — Terminal app idle and scrollback memory audit

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Costs below
are code budgets and slot/byte bounds read from current source, not fresh runtime samples. Every
change drops cached rendered state only: no cell, run, line, pane offset or output byte is altered,
so the drawn frame is identical (a hidden tab simply rebuilds its rows when shown again).

**Target:** `crates/flamewm-terminal/src` — scrollback storage (per-cell struct size, `Vec`
growth, default line cap), redraw on PTY idle, cursor blink timer wakeups when unfocused, and
per-tab buffers when hidden. `theme.rs`, `themes.rs` and `app.rs` are owned by another worker and
were read only, never edited.

## Owned files (identified and listed before any edit)

- `crates/flamewm-terminal/src/app/grid.rs` — per-frame pane cache ownership and pruning.
- `crates/flamewm-terminal/src/app/tabs.rs` — explicit tab close.
- `crates/flamewm-terminal/src/app/layout.rs` — split-pane removal and last-pane tab close.
- `docs/swarm/units/u31-terminal-mem.md` — this report.

Read-only evidence (not touched): `screen/cell.rs`, `screen/history.rs`, `screen.rs`, `store.rs`,
`session.rs`, `wake.rs`, `main.rs`, `ui.rs`, `view.rs`, `app.rs`, `theme.rs`, `themes.rs`.
No file outside the owned set was edited.

## Audit findings

- **Scrollback storage.** `Cell` (`screen/cell.rs:24-30`) is `char` (4 B) + `Color` (4 B) +
  `Color` (4 B) + `attrs: u16` (2 B) = 14 B payload, padded to 16 B at align 4; `attrs` is already
  one `u16` bit field (`cell.rs:11-22`), so there are no unpacked per-attribute booleans to merge.
  `Color` (`cell.rs:3-9`) is 4 B because `Rgb` carries three bytes plus a tag; packing it into a
  `u16` would rewrite every `Color::Rgb`/`Indexed` pattern match, including the other worker's
  `theme.rs`/`themes.rs`, and `#[repr(packed)]` risks unaligned field references in derived traits.
  Deferred, not applied. The hot path already avoids per-cell scrollback: `screen/history.rs:19-32,35-99`
  stores a uniform line as UTF-8 text with no run list and a mixed line as text plus one 16 B
  `StyleRun` per style, falling back to plain cells only when style churn makes packing lose.
  The cap is 10,000 lines by default (`store.rs:145`, clamped to 0..=1,000,000 at `:160-161`) and
  is enforced on both drain paths (`screen.rs:343-350`, `:444-452`). `VecDeque` grows geometrically
  while filling, then holds spare slots; calling `shrink_to_fit` at the cap would force a full
  reallocation on every later overflow push (push at `len == capacity` grows before the pop), so
  it is deliberately not done. Lowering the default cap would remove visible history, so it is
  also not done: output stays identical.
- **Redraw on PTY idle.** The reader parks in `poll` with no timeout when no wake is owed
  (`session.rs:223-228`) and coalesces wake bytes to one per 16 ms `FRAME` (`:173,210-216`).
  An empty wake queue commits no frame (`wake.rs:26,40`), a hidden window parses output but skips
  projection and projects once on uncover (`main.rs:266-271,297-304`), and `UiCache::project_row`
  skips rows whose runs are unchanged (`ui.rs:185-208`). No idle redraw exists, so no wakeup
  change was needed.
- **Cursor blink timer wakeups when unfocused.** A crate-wide search finds no blink timer: only
  the `screen.rs:1028` comment ("Blink is not stored: the renderer draws a steady shape") and a
  benchmark label in `ui.rs:669-670,741-742`. The sole periodic source is the scroll glide ticker,
  which sleeps only while a frame is requested and exits when its sender is dropped
  (`app/scroll.rs:12-33,39-57`). There is no unfocused blink wake to cancel; no change was needed.
- **Per-tab buffers when hidden.** `App::pane_runs` (`app.rs:144-145`) is a `HashMap<u64, RunCache>`
  keyed by session id. Each `RunCache` (`view.rs:235-257`) owns a full `Vec<Row>` of baked runs, a
  cloned `Palette` key, `stale: Vec<bool>`, `changed: Vec<usize>` and a decoded `Line` scratch.
  `grid_frame` inserted entries but never removed them (`app/grid.rs:170-198`), so one cache per
  pane ever drawn stayed resident for panes in hidden tabs and for closed splits. This is the one
  real retained-buffer leak found; it is now pruned.

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `app/grid.rs:196-200` | Before: one full `RunCache` per pane ever drawn, including every hidden tab's panes, held for the life of the window (baked rows + cloned palette + three scratch vectors per pane). | After each frame, `cache.retain(|id, _| tab.root.contains(*id))` keeps rows only for panes in the active tree; a hidden tab rebuilds identical rows from its unchanged screen when shown again. No allocation is added to the frame. | Low-medium: switching tabs now rebuilds that tab's rows once instead of reusing them, so the first frame after a switch costs a full row rebuild. Screen state (offset, selection, search) lives in `Pane`, not the cache, so the rebuilt frame is byte-identical. UNBUILT. |
| `app/tabs.rs:55-58` | Before: an explicit tab close left its panes' `RunCache` entries and the map's high-water bucket count until process exit. | On `close_tab`, retain only ids still present in the closing tree and call `pane_runs.shrink_to_fit()`, releasing both the entries and the spare buckets immediately. | Low: `HashMap::retain`/`shrink_to_fit` run only on a user tab close, not per frame; closed session ids are never reused (`next_id` only grows), so no live cache can be evicted. UNBUILT. |
| `app/layout.rs:484-489` | Before: closing a split pane, or the last pane of a tab (including on child exit), kept that pane's cache; the last-pane path also kept the map's buckets. | `remove_pane` drops the removed id's cache and calls `shrink_to_fit` when the tab becomes vacant, covering `close_pane` and the `SessionEvent::Exited` path that `close_tab` does not use. | Low: the id is already absent from the tree when the cache is removed, and `relayout` reuses live panes by id, whose caches are untouched. UNBUILT. |
| `screen/cell.rs:24-30`, `screen/history.rs:19-32,35-99` | 16 B/cell live; 10,000 plain lines would be ~12.8 MB per tab, but ordinary output is text + runs, not cells. | No change: attrs are already a single `u16`; scrollback packing is already in place. Packing `Color` into `u16` was rejected because it spans `theme.rs`/`themes.rs` (another worker) and all variant matches, and `#[repr(packed)]` is unsafe with derived field refs. | High if forced: a `Color` representation change would break the public enum used by palette code; deferred to the owning worker with a build in scope. |
| `screen.rs:48-49,343-350,444-452`; `store.rs:145,160-161` | `VecDeque<History>` capped at 10,000 lines; geometric spare capacity (~6.4k slots) is retained, but shrink-at-cap would realloc on every overflow push. | No change: growth is bounded, and lowering the cap would remove visible history. | None introduced; any cap change is a visible-output change and out of scope for "identical output". |
| `session.rs:173,210-228`; `wake.rs:26,40`; `main.rs:266-271,297-304`; `ui.rs:185-208` | Idle PTY has zero wakes; hidden windows skip projection; equal rows write nothing. | No change: no idle redraw or redundant commit found in the current source. | None introduced. |
| `app/scroll.rs:12-33,39-57`; `screen.rs:1028`; `ui.rs:669-670,741-742` | No blink timer exists; the glide ticker sleeps only while a frame is requested. | No change: there is no unfocused blink wake to stop. | None introduced. |

## Verification

UNBUILT by contract: no compiler, test runner, `git`, Python or sleep loop was invoked in this unit.
Identical visible output is argued from source only: the changes remove `RunCache` entries for ids
that are not in the active pane tree, and `RunCache::update` rebuilds all rows from `Screen` state
when a fresh entry is created, so a re-shown tab paints the same rows. Pane-local view offset,
selection and search state are stored on `Pane`, not on the dropped cache. The memory and rebuild
costs need the orchestrator's build, tests and a scoped tab-switch/tab-close sample to confirm.

PARTIAL — all four requested areas were audited against current source. The one real leak,
per-pane run caches for hidden tabs and closed splits, is fixed in the three owned source files;
the scrollback is already line-packed and capped, the PTY path already parks at zero idle wakes,
and no cursor blink timer exists to stop when unfocused. Cell-attribute packing was deliberately
deferred because `Color` is owned by another worker and changing it would alter visible palette
code and risk unaligned accesses. Everything is marked UNBUILT pending the orchestrator's build
and tests.
