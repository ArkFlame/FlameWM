# u34-terminal2 log

Reviewed the current state of the batch-4 terminal optimisations (batch history is unreadable here: `git` is
forbidden in this shared checkout, so the review is source-based): scrollback `History` packing, dirty-row tracking
with `Dirty::{All, Rows}`, row-level `RunCache` reuse, pooled UI projection, PTY batching/pacing, parser parameter
reuse. No functional regression found. One real defect fixed: two misplaced doc comments in `app/overlay.rs` (the
context-menu comment sat on `draw_find`). The banner plumbing is dead by design (`BANNER` is empty) but is a retained
feature toggle and is referenced by tests, so it was left in place rather than deleted.

Next worst hotspots by composition share and what changed:

- **Idle/repaint syscalls (per frame).** `App::frame` → `ghost()` → `at_prompt()` did two `tcgetattr` ioctls per pane
  on every repaint, although the line discipline only changes when the child writes or when a key is sent. Now
  `PtyProcess::line_modes()` returns `(canonical, echoing)` from one ioctl (replacing the now-deleted `canonical` and
  `echoing`), `Session` caches the pair, refreshes it per output batch (`Session::feed`) and per key press
  (`App::track_key`), and `at_prompt` reads the cache. Expected shift: -2 syscalls per pane per frame, and -1 ioctl per
  key press.
- **Ghost suggestion allocation (per frame).** `overlay::ghost` called `LineInput::text()`, cloning the typed line into
  a fresh `String` every frame at the prompt. Added `LineInput::as_str` and suggest from the borrow. Expected shift:
  one heap allocation removed per frame while a suggestion is shown.
- **Parser per-byte dispatch.** `Parser::advance` ran the full state machine (`utf8_need` check, escape/control
  checks, state match) for every byte. Added a ground-state fast path that emits a whole run of printable ASCII
  directly through `Perform::print`. Expected shift: the bulk of PTY output (build logs, `cat`, prompt echo) skips two
  branches and a match per byte; escape/UTF-8 sequences take the unchanged path.
- **Dirty-row list allocation (per frame).** `Screen::take_dirty` built a fresh `Vec<usize>` each frame.
  `take_dirty_into(&mut Vec<usize>) -> bool` now fills a caller buffer; `RunCache` holds that buffer in `changed`.
  `take_dirty` is kept for tests and delegates. Expected shift: no heap traffic per frame for the dirty list.
- **Grid row composition copies (per changed row).** `compose_row` cloned every run (and its `String` text) even for the
  common single full-width pane at column 0. It now hands back the pane's shared `Arc<[Run]>` when every run already
  fits the pane rectangle. Expected shift: no per-run text allocation when rows recompose (a scrolled flood
  recomposes every row each frame), and better `Arc::ptr_eq` reuse downstream in the UI cache.

RATIONALE: composition-first. Repaints are the steady-state cost of an idle terminal, parsed bytes are the cost of a
busy one, and row recomposition is the cost of scrolling output; each change removes syscalls, allocations or copies
from exactly those paths without changing any drawn cell.

No cargo/git was run (shared checkout: the orchestrator builds and commits).
