# u17 — Wallpaper retained raster cut, hotlist rank 3

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run; the
orchestrator builds/tests/commits after the batch lands. Pixels are unchanged by construction:
every cut reuses the same decode result or drops a buffer that nothing visible samples.

## Owned files (identified before editing)

- `crates/flamewm-image-core/src/decode_cache.rs` — decode cache, source of the retained raster.
- `crates/flamewm-image-core/src/wallpaper.rs` — wallpaper decode API facade.
- `crates/flamewm-lock/src/document.rs` — lock per-output background decode.
- `crates/flamewm-desktop/src/wallpaper_loader.rs` — desktop wallpaper loader lifecycle.

Explicitly not touched: `crates/flamewm-desktop/src/widgets/**` and
`crates/flamewm-shell/src/projection/**`. `crates/flamewm-render-x11/src/backdrop/**` was inspected
but left unchanged: it already drops the natural raster after composing a small canvas, so caching
the full raster there would be a regression.

## Applied cuts

1. One decoded buffer per unchanged source in a process. `decode_cache::decode_file_shared` keys a
   process-local `Arc<RgbaImage>` by the same `(path, len, mtime)` tuple as the on-disk entry. At
   most one full-resolution raster is held: a new source clears the old handle. The lock now
   borrows that handle for every output and every re-show instead of re-reading and re-decoding
   the same file; other processes keep using the existing on-disk decode cache, so desktop and
   lock still start from identical decoded bytes without a second decode.
2. Drop the retained raster when a solid fill covers the desktop. `request_solid` and loader
   shutdown clear the process-local decoded handle in addition to dropping the loader's own
   `applied` request and presentation. A later image request re-reads the unchanged on-disk cache
   and produces identical pixels.
3. Already landed before this unit and preserved: the on-disk decode cache (unchanged path+len+
   mtime is decoded once across processes), the small downscaled backdrop canvas, and the lock
   built-in background `OnceLock`. No duplicate decode path was added.

## Log rows (file:line, cost, change, risk)

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-image-core/src/decode_cache.rs:43-87` | One extra `HashMap` + `Mutex` (one entry, ~8-9 MiB at 1080p) | Added `MemoryKey` and bounded process-local `Arc<RgbaImage>` reuse keyed by path, length and mtime; a new source evicts the old raster | Low: lock is held only for map access; poison is recovered; cache failures fall back to the existing decode; maximum retained raster count is one |
| `crates/flamewm-image-core/src/decode_cache.rs:90-101` | Repeated lock-output decode read (~8 MiB per call) | Added `decode_file_shared`, reusing the in-process handle before falling back to `decode_file_cached` | Low: identical decode result; an edited file changes the key, so no stale pixels |
| `crates/flamewm-image-core/src/decode_cache.rs:105-109` | Retained raster while desktop is solid or loader is down | Added `release_shared_raster` | Low: next request decodes the same on-disk entry again |
| `crates/flamewm-image-core/src/wallpaper.rs:41-43` | None | Exposed `decode_file_shared` through the wallpaper facade | Low: wrapper only |
| `crates/flamewm-lock/src/document.rs:217-220` | One full decode/read per output and per lock show | Lock custom background borrows the shared decoded raster instead of re-decoding it | Low: `cover_rgba` only borrows; output pixels unchanged |
| `crates/flamewm-desktop/src/wallpaper_loader.rs:248-251,349-350` | Full-resolution raster retained after solid cover / at teardown | Release the process-local handle on solid fill and shutdown | Low: a later image request re-reads the same cached decode |

## Not done

The "desktop covered by other windows" signal lives in
`crates/flamewm-desktop/src/widgets` and `crates/flamewm-desktop/src/main.rs`, outside this
unit's owned files; wiring it into the loader would need a cross-file contract change and is left
to the orchestrator. No build, test or runtime measurement was performed (UNBUILT), so no speed or
memory reduction is claimed as measured; only the code-shape change is evidenced.

PARTIAL — shared decoded buffer and covered/teardown release are applied in the wallpaper decode
path, and unchanged path+mtime reuse now avoids both re-decode and re-read inside a process; the
covered-by-windows hook is deferred because its signal owner is outside this unit, and all changes
remain UNBUILT with no measured result.

NOTE u17b: decode_file_shared (+80 LOC in flamewm-image-core) reverted: architecture gate core-source-loc>2400. Redo inside flamewm-desktop or after trimming image-core.
