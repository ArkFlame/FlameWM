# LOG u103-taskmgr-render — task manager render, batch fifteen

Area: `crates/flamewm-task-manager/src/**` (table paint, per-row text, graph rasters).

## Audit against the unit brief

- **Draw only visible rows — already in place.** `paint_table` computes `shown` from `viewport`,
  the remaining hits and `ROWS`, emits `visible` only for slots that enter or leave that window
  (`painted_rows`), and iterates `0..shown`. The always-empty tail of the 40 slots costs nothing.
- **Graph ring buffers with fixed capacity — already in place.** `graph::History` is a fixed
  `[f32; SAMPLES]` ring with an O(1) push, `render_into` reuses the caller raster, and the three
  cards ping-pong two owned `Arc<Vec<u8>>` buffers per card, so a visible sample allocates no
  raster. No change was needed; the earlier slice kept the pixel output identical.
- **Cache row text until value changes — this slice.** The table used to re-format every visible
  cell on every paint: name/indent, pid, threads, priority, nice, virt, cpu `{:.1}%`, mem, cmd.
  Paints happen on every sample *and* on every wheel scroll, header click and key repeat, so a
  scroll burst paid the same formatting work as a new sample even though the numbers were equal.

## Change

`RowText` (one per visible slot, `Vec<RowText>` of length `ROWS`) now owns the formatted cell
strings and the raw values they render. `update(process)` rebuilds only the cells whose raw value
moved:

- name cell on name, command or depth change (`display_name` reads name + command, so both count);
- pid cell on PID/start-time identity change;
- threads, priority, nice, virt, cpu, mem each on their own value.

Raw string identity is stored as `Arc<str>` clones, so the change test is a `str` compare and
keeping the value costs a refcount bump, not a heap copy. Cell strings keep their capacity across
rebuilds (`clear` + `write!`), so a changing CPU column allocates nothing after the first sample.

`paint_menus` already short-circuits when no menu is open, and hover motion patches only the two
affected row backgrounds, so the remaining per-paint work is the document calls themselves
(`text_ref` dedupes unchanged text internally).

## Expected composition shift

With the window visible the sampler worker still owns nearly all of the task-manager share
(`/proc` reads at 1 Hz). On the UI thread the per-sample table cost drops to the ~4–6 cells per row
that actually change; for interaction repaints (wheel scroll, sort clicks, key repeat) the whole
row formatting set is skipped, leaving only the document lookup per cell. Graph raster work is
unchanged (~40k pixel writes per sample for the three cards) and remains the largest single render
item, so it is the next candidate if this area is revisited.

## Tests

Added `row_text_cache_rebuilds_only_changed_cells`: formats one process, asserts the cells, then
verifies a name change rebuilds only the name cell while the CPU text is reused, a CPU change
rebuilds the CPU cell, and a new PID/start identity rebuilds the row.

No cargo/git executed in this unit (shared checkout); the change was compile-reviewed by reading
(disjoint field borrows only: `snapshot`, `rows`, `row_text`, `selected`/`hovered`).
