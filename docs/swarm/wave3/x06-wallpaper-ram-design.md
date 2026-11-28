# x06 design: output-sized wallpaper decode cache

Route evidence: workspace version 0.1.7 (`Cargo.toml:6`; `scripts/version` derives the same value
through Python, which this brief forbids, so the manifest was read instead). Routes from
`.opencode/ROUTES.md` and `.opencode/HARNESS.md`: source-first (owner and callers),
reference-mining (`.vendor/**` evidence only), rust-systems (ownership, drop and teardown),
semantic-modularization (single typed facade), parallel-execution (this checkout is shared).
The task brief fixes the read budget and forbids Cargo/Git/Python, so no build or test was run.

## Current structure (file:line)

The decode path has one semantic owner, `flamewm-image-core`, behind two facades.

- `crates/flamewm-image-core/src/decode_cache.rs:29-40` — `decode_file_cached`: decodes at
  natural size with `decode_file_uncached` and caches that raster on disk.
- `crates/flamewm-image-core/src/decode_cache.rs:42-56` — `MemoryKey` and a one-entry in-process
  map; `:74-87` `memory_get` / `memory_put` (`memory_put` clears the map, so at most one raster
  is resident); `:89-102` `decode_file_shared`; `:104-112` `release_shared_raster`.
- `crates/flamewm-image-core/src/decode_cache.rs:122-137` — `entry_for`, keyed only by
  (path hash, byte length, mtime); `:139-145` `cache_dir`; `:147-164` `read_entry`;
  `:173-198` `write_entry`; `:200-255` `prune` (byte cap 64 MiB at `:25`, entry cap 256 at `:27`).
- `crates/flamewm-image-core/src/wallpaper.rs:33-52` — `decode_file`, `decode_file_shared`,
  `decode_file_uncached`; `:54-100` `compose`; `:102+` `compose_region` / `compose_region_into`.
- `crates/flamewm-image-core/src/lib.rs:242-255` — `decode_bytes`; `:257-281` `decode_bytes_at`
  decodes natural size first for Png/Webp/Jpeg/Gif and then scales; only Svg renders at target.
- `crates/flamewm-desktop/src/wallpaper_loader.rs:483-506` — `decode_bytes_at`: a block scope
  already drops the natural raster after composition; `:517-535` `compose_decoded`;
  `:537-578` `compose_per_monitor` composes directly into one root canvas.
- Release callers: `crates/flamewm-desktop/src/main.rs:463`,
  `crates/flamewm-desktop/src/wallpaper_loader.rs:253,361`,
  `crates/flamewm-lock/src/document.rs:258`.
- Other consumers decode at natural size:
  `crates/flamewm-render-x11/src/backdrop/canvas.rs:255`,
  `crates/flamewm-render-x11/src/backdrop/desktop.rs:205`.

## Counted problems and gaps

- **Cache granularity.** `entry_for` (`decode_cache.rs:122-137`) keys only on source identity, so
  every cached entry is the *natural-size* raster. A 3840x2160 source saved for a 1920x1080 root
  spends 3840*2160*4 = 33,177,600 bytes per entry where 1920*1080*4 = 8,294,400 bytes are needed:
  **4x** the RAM on a cache hit and 4x of the 64 MiB byte budget. Counted from the dimensions, not
  measured on a live display (this brief forbids builds/runtime).
- **No output-size cache API.** `decode_bytes_at` (`lib.rs:257-281`) re-decodes the whole raster
  for four of seven formats, and `wallpaper_loader::decode_bytes_at` (`:486-506`) calls
  `decode_file` (natural) on every miss or cold start. Counted: **0** callers can request an
  output-sized cached wallpaper.
- **Unused resident copy.** `decode_file_shared` (`decode_cache.rs:92-102`) keeps one natural-size
  `Arc<RgbaImage>`; `rg` finds no production caller of the facade at `wallpaper.rs:41-43`.
- **Test coverage.** `decode_cache.rs:257-366` has **5** tests (roundtrip, stale sibling, LRU by
  bytes, keep-on-overflow, entry cap); **0** cover target-size or fit keying.

## Reference precedent and choice

- `.vendor/xfdesktop-master/src/xfdesktop-backdrop-renderer.c:505-552`
  (`loader_size_prepared_cb`) calls `gdk_pixbuf_loader_set_size(loader, w, h)` for
  stretched/scaled/zoomed/spanning styles: the decoder is told the screen size before pixels are
  materialised, so the full-resolution source is never the working buffer. The renderer then
  composites into a screen-sized `final_image` (`:302-500`).
- **Choice:** port the *semantic* rule — cache key includes the requested output dimensions and
  composition, and the cached raster is output-sized — not the code. No GdkPixbuf, cairo or
  vendored file is used; FlameWM keeps `compose` as the single pixel authority and reuses its
  existing in-process scale. No verbatim code was copied.

## Target design

Keep `flamewm-image-core::{decode_cache, wallpaper}` as the only owner of wallpaper decode and
its caches. A cache entry is identified by (source path hash, byte length, mtime, output width,
output height, fit code) and stores exactly the composed, output-sized RGBA8 raster. A hit returns
only output-sized pixels; a miss decodes the source inside one call scope, composes with the
existing `compose` (pixel-identical), writes the output-sized raster, and drops the source before
returning. The existing 64 MiB / 256-entry `prune` and the one-entry in-process map stay the only
bounds; no new bound or dependency is introduced. Producers pass their target size through the
typed facade; native consumers never hold source-sized buffers as authority.

## Steps (each compiles alone)

1. **This unit.** Add `decode_cache::decode_file_composed_cached(path, width, height, fit)` and its
   `wallpaper::decode_file_composed` facade, plus tests for output-size/fit keying and roundtrip.
   No production caller changes, so behavior and pixels are unchanged. **UNBUILT** here.
2. Wire `wallpaper_loader::decode_bytes_at` to the new facade for the single-full-root case,
   keeping `compose_per_monitor` for multi-monitor roots. Compiles alone on step 1; pixels stay
   identical because `compose` is reused.
3. Wire the shell/pseudo-transparency consumers (`render-x11/backdrop/canvas.rs:255`,
   `desktop.rs:205`) to request their own target size and keep the existing
   `release_shared_raster` calls on cover/teardown. Compiles alone on step 2.
4. Only if a scoped perf test shows decode is the largest share, add decoder-level downscale for
   raster formats behind the same facade. Any such change must be proven pixel-identical or
   explicitly approved before it lands. Compiles alone.

## Rollback

Step 1 reverts by deleting `decode_file_composed_cached` and `composed_entry` from
`decode_cache.rs`, the `decode_file_composed` facade from `wallpaper.rs`, and the new tests; no
existing function signature or call site changes, so nothing else compiles differently. Steps 2
and 3 revert independently by restoring their previous call sites. No new dependencies are added.

## Step 1 result (UNBUILT)

Production edits: two, both inside the `flamewm-image-core` typed facade.
`decode_cache.rs` gains `composed_entry` and `decode_file_composed_cached` (source identity plus
target width/height/fit in the entry name, composed raster written through the existing bounded
`write_entry`/`prune`); `wallpaper.rs` gains the `decode_file_composed` facade. No caller is
changed, so visuals remain pixel-identical. Tests in `decode_cache.rs` exercise the new key and a
write/read roundtrip without needing a display or a real image file. All edits are **UNBUILT**;
no Cargo, Git, Python or build command was permitted in this unit.
