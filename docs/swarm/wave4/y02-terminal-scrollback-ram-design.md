# y02 — Terminal scrollback RAM

## Current structure (file:line)

- `crates/flamewm-terminal/src/screen/cell.rs:24-30,44` — `Cell { ch, fg, bg, attrs }` and
  `pub type Line = Vec<Cell>`: the live-grid unit, one value per column.
- `crates/flamewm-terminal/src/screen/cell.rs:46-53` — `history_len` drops trailing
  default-blank cells but keeps one cell so `line[0]` stays valid; this is the blank-tail
  compression boundary.
- `crates/flamewm-terminal/src/screen/history.rs:10-32` — `StyleRun` and the
  `History { Plain(Line) | Uniform { text, fg, bg, attrs } | Packed { text, runs } }`
  enum, the three retained forms; `:36-99` picks the smallest form, `:114-144` decodes it
  back into cells with one reusable buffer (`cells_into`), so stored text always restores
  the exact original cells.
- `crates/flamewm-terminal/src/screen.rs:48-49,85` — the ring and its bounds:
  `scrollback: VecDeque<History>`, `scrollback_limit: usize` and the absolute-index
  renumber counter `scrollback_drops: u64`; `:97,106` construct them.
- `crates/flamewm-terminal/src/screen.rs:241-253` — the read facade
  (`scrollback_len`, `scrollback_drops`); no caller reaches the `VecDeque` directly.
- `crates/flamewm-terminal/src/screen.rs:258-264` — `push_history`, the single append
  point introduced by step 1: append, enforce the line cap once, bump `scrollback_drops`.
- `crates/flamewm-terminal/src/screen.rs:343-354` (resize drain) and `:434-453`
  (`scroll_up` live scroll) — the two producers, both routed through `push_history`.
- `crates/flamewm-terminal/src/screen.rs:954-959` — `CSI J` mode 3 clears saved lines and
  increments the same renumber counter once.
- `crates/flamewm-terminal/src/screen/tests/mod.rs:245-263` — the existing cap tests
  (`scrollback_is_capped`, `shrinking_grid_obeys_scrollback_cap`); `:266-283` is the new
  cap test that drives compressed, blank-trimmed lines through the ring.
- `.vendor/terminator-master/terminatorlib/config.py:248-249` and
  `terminatorlib/terminal.py:911-915` — the named reference: a configured
  `scrollback_lines` count with an `scrollback_infinite` sentinel resolved to `-1` before
  `vte.set_scrollback_lines(...)`.

## Counted problems

1. **The cap had no single owner.** The identical "append, drop oldest, renumber absolute
   lines" block was written out at `screen.rs:346-349` (resize drain) and `:448-451`
   (live scroll), with a renumber-only variant at `:954-959`. Count: three hand-maintained
   copies of one invariant; adding a byte budget would have made it four.
2. **No byte bound.** The only retention bound is a line count (`screen.rs:49`). A
   `Plain(Line)` entry can cost `cols * size_of::<Cell>()` bytes; with 240 columns and a
   10,000-line limit one screen can retain roughly 38 MiB. Count: one unbounded dimension
   (bytes) per `Screen`, with no accounting accessor in `screen/history.rs`.
3. **Cap tests did not cover compression.** `screen/tests/mod.rs:245-263` asserted only the
   line count and the oldest-line identity; no test drove a `History`-packed, blank-tail
   trimmed line through eviction and decoded the survivors. Count: zero tests covering the
   interaction of compression, trimming and the cap.

## Reference precedent (path + choice)

- `.vendor/terminator-master/terminatorlib/config.py:248-249` — defaults
  `scrollback_lines = 500`, `scrollback_infinite = False`.
- `.vendor/terminator-master/terminatorlib/terminal.py:911-915` — resolves the pair to
  `set_scrollback_lines(-1)` for infinite scrollback, otherwise to the configured line
  count.
- Choice: port the **explicit, configurable line-count bound** and treat "unbounded" as a
  deliberate sentinel rather than an accident. FlameWM keeps the bounded side only:
  `scrollback_limit` is a hard line count and no infinite mode is added, because
  unbounded retention defeats the RAM goal. The ring itself stays local
  (`VecDeque<History>`) and no VTE code, GTK dependency or vendor asset is copied.
  Because `cells_into` restores exactly the original cells, rendered pixels stay
  pixel-identical.

## Target design

- One owner per mechanism: `Screen::push_history` (`screen.rs:258`) is the only place that
  appends to the ring, enforces the line cap and bumps the renumber counter. Both
  producers delegate to it, so the bound cannot drift between paths.
- Step 2 introduces a typed `Scrollback` facade that owns `VecDeque<History>`, the line
  limit, the drop counter and an exact running byte total. `Screen` delegates `push`,
  `get`, `pop_back`, `clear`, `len` and `drops` to it, replacing the remaining inline
  clear site. A byte budget is enforced on the same push path as the line count.
- Step 3 extends the `scrollback_drops` renumber contract to byte eviction so a renderer
  that reuses rows after eviction sees a consistent absolute index.

Blast radius: `flamewm-terminal` internals only. No WM authority, reactor ownership or X11
coordinate/grab rule changes; the UI remains a typed projection and visuals stay
pixel-identical. No new dependencies.

## Steps (each compiles alone)

1. **Implemented here (UNBUILT):** add `packed_cost` (`history.rs:37`) and
   `History::charged_bytes` (`history.rs:121`) so admission and any later RAM ledger share
   one cost model, add the private `Screen::push_history` helper
   (`screen.rs:258-264`), route both the resize drain (`:354`) and live scroll (`:452`)
   through it, and add three tests: the two cost-model tests (`history.rs:210,249`) and
   `capped_ring_keeps_two_newest_compressed_lines_and_trims_blank_tails`
   (`screen/tests/mod.rs:266-283`). Compiles alone, adds no dependency, changes no pixel.
2. **Planned:** add the typed `Scrollback` facade with line + byte budgets and migrate the
   remaining clear site and the read facade to it. Compiles alone.
3. **Planned:** extend the `scrollback_drops` contract to byte eviction and cover it in
   `screen/tests/mod.rs`. Compiles alone.

## Rollback

- Step 1: restore the two inline push/pop/drop blocks at `screen.rs:343-350` and
  `:434-453`, delete `push_history`, `packed_cost`, `charged_bytes` and the three new
  tests, restoring the two inline cost expressions in `pack`. No state or data migration,
  no runtime identity change, no visual change.
- Step 2: restore the raw `VecDeque<History>` fields and the three inline cap sites.
- Step 3: restore the line-count-only renumber contract.
- All steps keep visuals pixel-identical and add no dependencies.
