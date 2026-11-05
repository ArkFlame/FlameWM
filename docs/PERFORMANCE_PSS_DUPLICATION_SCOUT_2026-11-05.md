# PSS duplication scout (2026-11-05)

Scope: per-process duplicated allocations across UI processes (fonts, icon themes, settings, skins).
Status: static scout only. No nested session was run, so no before/after PSS (`/proc/<pid>/smaps_rollup`) numbers exist. Runtime claims are unverified.

## Findings (ranked by static evidence)

1. **Parsed fontdue `Font` per process (wm, lock).** Each process that rasterizes text parses the TTF once. `flamewm-wm-wayland/src/font.rs` (`load`, `FONT` static) and `flamewm-render-core/src/glyphs.rs` (`find_font`, `FONT` static) each hold a `fontdue::Font` with glyph outlines, `char_to_glyph`, and the kern table. These are anonymous heap pages and are not shared across processes. Size is unmeasured; IBMPlexSans-Regular.ttf is 192,980 bytes on disk, and parsed size is typically a multiple of that. Only `wm-wayland` and `lock` load fonts. The terminal crate has its own non-fontdue glyph module.
2. **Duplicate font-search and load code in two crates.** `wm-wayland/src/font.rs` and `render-core/src/glyphs.rs` implement the same candidate list, directory walk, and `Font::from_bytes`. They are not double-loaded in one process, because `wm-wayland` does not use `render-core::glyphs` (it only imports `render-core::outline`). Consolidating them would be a refactor, not a PSS win.
3. **Raw font bytes (`std::fs::read` into `Vec<u8>`): not a steady-state cost.** `fontdue` 0.9.5 `Font::from_bytes<Data: Deref<Target=[u8]>>` parses and does not retain `data` (the `Font` struct has no byte field). The `Vec` is dropped when `from_bytes` returns, so mmap would only remove a transient ~190 KB anonymous peak during startup. It would not reduce steady PSS. Not implemented.
4. **Icon themes, settings, skins.** Not scouted in this pass. The icon service split is owned by the architecture worker and was excluded. Settings are read per process with `read_to_string` and dropped; no steady-state duplicate was found in this static pass.

## Not done (and why)

- mmap via `memmap2` (already in `Cargo.lock` 0.9.12): no steady-state gain per finding 3. Rejected under the >=3% same-process A/B rule, since no A/B was possible.
- Lazy font init: `wm-wayland` already loads lazily on first `with_glyphs` call. `render-core` `FONT` is also lazy.
- Arc-sharing font tables across processes: not possible without a shared-memory owner. Needs an architecture decision.

## Next measurement (required before any change)

1. Build `flamewm-wm-wayland` with `CARGO_BUILD_JOBS=2` into a private target dir.
2. Start a nested session, sample `grep -E 'Pss|Rss' /proc/<pid>/smaps_rollup` for wm and lock before and after first text paint.
3. Compare the fontdue heap delta against total PSS. Only consider a font-side change if that delta is >=3% of the process PSS.
