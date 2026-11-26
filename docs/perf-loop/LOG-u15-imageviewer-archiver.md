# u15-imageviewer-archiver changes

- archiver/progress.rs: `Reporter` now coalesces progress emissions instead of forwarding every
  read chunk. The sink is called at most ~100 times per operation: on a `max(total/100, 64 KiB)`
  byte step, on a `max(total/100, 1)` entry step, and once on the completion snapshot (a cancelled
  or failed job simply emits less). `copy_tracked` used to run `io::copy` through a `Tracked`
  reader, so a 1 GiB entry produced ~131k sink calls, ~131k channel messages and ~131k UI wake
  bytes; each wake made the archiver window drain its channel and repaint the whole document
  (name, page, 12 rows, progress bar). Same archive: ~100 wakes and ~100 repaints. The bar still
  moves in 1% steps and the final snapshot still reports the exact totals, which the tests that
  assert `bytes_done == bytes_total` / `entries_done == entries_total` keep proving.
- archiver/progress.rs: `copy_tracked` hand-rolls the copy loop with a 64 KiB buffer instead of
  `io::copy`'s 8 KiB default, and checks cancellation between chunks. Extract of a file entry and
  the zip create path now need 1/8 of the read/write syscall pairs for the same bytes; `Interrupted`
  is retried, cancel still maps to `ArchiveError::Cancelled` and a partial file is still removed.
- archiver/extract.rs: `write_file` no longer walks `dest` plus the entry path with `lstat` for
  every entry. `prepare_parent` checks and creates the entry's parent once and remembers it, and a
  directory entry registers itself as the next ready parent. A 100k-entry archive with 3 path
  components used to pay ~500k `lstat` calls and one `create_dir_all` (2-4 syscalls) per file; now
  one walk per directory plus one syscall for the entry itself (`symlink_metadata`, which already
  rejects symlink/hardlink/device targets). Safety properties are unchanged: the destination
  pre-check, `reject_symlinks` on each first parent, and the per-target metadata check all remain,
  and the pre-existing-symlink / hardlink / zip-slip tests still take the `UnsafePath` path. The
  not-atomic-with-concurrent-modification caveat in the module doc already covered this.
- imageviewer/decode.rs: `crop` computes the source column offsets once per crop (one `Vec<usize>`
  the width of the visible rect) instead of two integer divisions per output pixel, and copies
  4-byte pixels out of the source row through that map. Panning and resizing produce one crop per
  visible frame (up to stage size, 1000x680 = 680k pixels), so this removes ~1.36M 64-bit divisions
  per pan frame and keeps the nearest-neighbour mapping identical.
- image-core/decode_cache.rs: `read_entry` drops the 16-byte header with `Vec::drain` instead of
  `split_off`, which allocated a second raster-sized buffer and copied the whole image before the
  read buffer was freed. Reading a cached 4K wallpaper raster (33 MiB) now allocates once instead
  of twice at peak.
- image-core/scale.rs: `scale_area` builds its per-axis coverage taps as three flat buffers
  (`first`, `offsets`, `weights`) instead of one `Vec<f32>` per destination index per axis. A
  4000x3000 photo fitted to a 1000x680 stage used to allocate ~2 * 1000 short vectors per scale
  call (and a scale call happens on every zoom step and on wallpaper resize); it now allocates six.
  `push_area_tap` keeps the exact `f64` coverage arithmetic and `f32` normalisation order of the
  public `area_tap`, so wallpaper golden output and the frozen odd-ratio pixel tests are unchanged;
  `area_tap` itself is now a thin wrapper.

Expected composition shift: the archiver stops dominating UI wakes and full-window repaints while
extracting or creating large archives (the prototype case: one big file), and stops spending a
syscall storm on path checks for many-file archives; image viewer pan/zoom loses per-pixel divides
and per-tap allocations, and the wallpaper/blur scale path allocates two orders of magnitude fewer
times. Visuals, output bytes, ordering and error kinds are unchanged.
