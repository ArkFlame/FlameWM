# u17c — Wallpaper pipeline retained raster RAM audit (hotlist rank 3)

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every cost
below is deterministic RGBA byte arithmetic or a figure already cited in `docs/PERF_HOTLIST.md`
and `docs/history/FLAMEWM_PASS46_WALLPAPER_PEAK_MEMORY_AUDIT_2026-10-20.md`, not a fresh RSS sample.

**Scope:** `crates/flamewm-desktop/src/wallpaper_loader.rs`,
`crates/flamewm-image-core/src/{decode_cache,wallpaper}.rs`, `crates/flamewm-lock/src/document.rs`
(read-only evidence also from `flamewm-lock/src/{main,run,settings}.rs` and
`flamewm-ui-document/src/template.rs`). Hotlist row 3 is `docs/PERF_HOTLIST.md:17`: a retained
root raster of ~8–9 MiB at 1080p, ~9.77 MiB (25 MiB portrait) 4K cropped-Fill staging, and a cold
lock build of 536 ms with decode ~53%, cover ~26%, blur ~14%
(`docs/perf-loop/LOG.md:379,382-384`). No source file was edited.

## Check answers

1. **Is the decoded source dropped after the cropped/scaled raster is built?**
   Desktop: yes. `wallpaper_loader.rs:480-481` decodes to an owned `RgbaImage` and moves it into
   `compose_decoded`, which either returns that same buffer (`:512-514`) or only borrows it for
   composition (`:516`, `:518`) before dropping it on return. Lock: no. `document.rs:217-221`
   borrows an `Arc<RgbaImage>` that `decode_cache.rs:90-99` also stores in the process-local
   `memory()` map (`:51-53`, `:79-85`); `cover_rgba` (`:234-253`) uses it by reference and the map
   entry lives until process exit. The built-in path is the same by design: `document.rs:197-202`
   holds the natural-size decode in a `OnceLock`. These are caches, not duplicates of the
   presented raster.
2. **Is the retained raster stored at output size?** Yes. Desktop `WallpaperRaster` takes
   `width/height/pixels` from the composed image (`wallpaper_loader.rs:285-290`), verified by
   `decoded_image_is_composed_to_root_size_and_refit_on_same_path` (`:973-1004`): a 2×1 source is
   presented as 4×2. Lock `RuntimeImage::from_rgba` wraps the `cover_rgba` result, which is exactly
   `width × height` (`document.rs:218-226`, `:247-252`), and crops are output-sized (`:159-192`).
   Only the two shared caches above hold source-size pixels, and neither is a presented raster.
3. **Is it released when the lock screen is not active?** Yes, by process teardown. `flamewm-lock`
   is its own binary: `main.rs:101-107` calls `run::run` once and returns an `ExitCode`, and
   `run.rs:78-86` returns from `lock_on` on successful authentication. The decode-cache map, the
   built-in `OnceLock`, every `LockSurface.base_background` (`run.rs:102-107,124-155`) and the
   Wayland blurred-background cache (`main.rs:132-155`) are all freed at process exit; there is no
   in-process "locked → unlocked" state that keeps them resident. Desktop's only explicit frees
   are `wallpaper_loader.rs:248-251` (solid fill) and `:350` (shutdown); desktop never populates
   the shared map (it uses `decode_file_cached` via `wallpaper.rs:35-37`), so those calls are
   defensive. Desktop's own output raster lives in `self.state` (`:146`) for the process lifetime,
   which is required to repaint after the lock is dismissed.
4. **Are two copies held at once during a wallpaper change?** Transiently, yes, but never two
   *retained* root rasters. `poll()` builds the new `WallpaperState` before assigning it over the
   old one (`wallpaper_loader.rs:283-293`), and a worker completion is held in
   `WorkerState.completion` (`:107`, `:402`) until `poll()` takes it. In the lock, `cover_rgba`
   holds the oversized `scaled` image and the output-sized `pixels` crop simultaneously
   (`document.rs:243-251`): ~9.6 MiB staging + 7.91 MiB output for the built-in 2560×1752 →
   1920×1080, and ~25.0 MiB staging for a 3840×2160 → 1080×1920 crop (the history doc's 9.77 /
   25.00 MiB figures). `shared_root_background` also coexists with one cropped
   `base_background` per output (`run.rs:124-155`), and the Wayland cache clones the blurred
   `RuntimeImage` while applying another copy (`main.rs:147-155`), because `RuntimeImage.pixels`
   is a plain `Vec<u8>` (`ui-document/template.rs:167-175`). `decode_cache::memory_put` clears the
   previous handle before inserting a new one (`decode_cache.rs:79-85`), so shared sources do not
   accumulate across changes.

## Rows — file:line, MiB, change, risk

| file:line | MiB | change | risk |
| --- | --- | --- | --- |
| `wallpaper_loader.rs:480-481,504-520` | 0 retained: natural source is moved and dropped after compose | None — already dropped; the equal-size path returns the same allocation, so no copy is made | Low, no edit. Adding a release here would be dead code. |
| `wallpaper_loader.rs:285-290` | ~7.91 MiB at 1920×1080, 8.79 MiB at 1920×1200 (hotlist's "~8–9 MiB") | None — already the composed output size, held in one `Arc<Vec<u8>>` shared with the UI document | Low, no safe smaller raster without changing rendered pixels or repaint behaviour. |
| `wallpaper_loader.rs:248-251,338-350` | 0 extra — explicit `release_shared_raster()` on solid/shutdown | None — already present; it frees the shared map in the desktop process | Low, no edit. It cannot free the lock process's map (separate process). |
| `decode_cache.rs:51-53,79-85,90-110` | Up to 31.64 MiB (3840×2160×4) for one custom source; one entry only | Cut A considered and rejected: clearing the map after `cover_rgba` would free this, but would force a re-decode per output and per lock show | Medium/high CPU regression: ~280 ms decode (53% of the 536 ms cold build, `perf-loop/LOG.md:379`) repeats; pixels would be identical, latency would not. |
| `document.rs:197-202` | 17.11 MiB (2560×1752×4 built-in WebP), process lifetime | None — the `OnceLock` is deliberately reused across outputs and re-shows after DPMS/suspend | High if cleared: re-decode on every output and lock show. No release hook exists without new ownership plumbing. |
| `document.rs:212-221` | Up to 31.64 MiB (custom 4K source), process lifetime | None — `decode_file_shared` is intentional; `share_wallpaper` defaults to `false` (`settings.rs:93`), so each output borrows the same handle | High if dropped after scaling: N output decodes per lock show instead of one. Pixels identical, CPU worse. |
| `document.rs:234-253` | ~9.6 MiB staging at 1920×1080; 25.0 MiB for 1080×1920 portrait; transient | None — the pixel-equivalent `wallpaper::compose_region` uses `ScaleFilter::Area` (`wallpaper.rs:80-85,347-363`), while `cover_rgba` uses `Bilinear` (`:243`); substituting it changes pixels, so it is not a safe cut | High — pixel change; a bilinear region compositor would be a larger unverified change (see the prior audit's "deliberately not changed" list, `FLAMEWM_PASS46_...md:25-26`). |
| `document.rs:159-192`; `run.rs:102-107,124-155` | Root canvas ~7.91 MiB + one output-sized crop per output (each up to ~7.91 MiB) | None — root-plus-crops is required to cover stale monitor geometry; each crop is a real UI raster | Medium — dropping the root or the coverage surface can expose the desktop. |
| `main.rs:132-155`; `template.rs:167-175` | One output-sized blurred copy cached + one applied copy (`Vec<u8>` clone) | None — the cache removes ~200 ms of blur per keystroke/minute tick (`perf-loop/LOG.md:374-376`); it is dropped when `wayland::run` returns on unlock | High if removed: repaint latency regression. No change needed for unlock cleanup. |
| `main.rs:101-107`; `run.rs:78-86` | 0 after unlock — process exit frees every raster above | None — Cut B rejected: there is no lock-inactive-but-resident state to hook, so an explicit free would be dead code | Low risk but zero benefit; adding an unlock hook would only duplicate OS teardown. |

## Cut decisions (at most two, none applied)

- **Cut A — drop the decoded source after scaling:** not safe as a pixel-identical, performance-neutral
  cut. Desktop already does it (`wallpaper_loader.rs:480-481,504-520`). In the lock, the surviving
  source is the process-local decode cache (custom) and the built-in `OnceLock`; both exist
  specifically to remove a ~280 ms decode from every additional output and every lock re-show
  (`document.rs:194-196,213-216`). Releasing after the first cover preserves pixels (the same
  on-disk entry is re-read) but reintroduces that decode on each subsequent consumer, with
  `share_wallpaper` false by default (`settings.rs:93`). The CPU cost is measured; the RAM saving is
  only arithmetic, so the trade is not justified here.
- **Cut B — free on unlock:** not applicable. `flamewm-lock` exits after authentication
  (`main.rs:101-107`, `run.rs:78-86`), so every retained raster is already released; the desktop
  frees its shared handle on solid/shutdown (`wallpaper_loader.rs:248-251,350`). An explicit unlock
  free would be unreachable in any state where the pixels are still resident.

## Verification

UNBUILT by contract: no compiler, test runner, `git` or Python was invoked, and no source file was
edited. All four checks are answered from current source read in this pass; line numbers were
re-read immediately before writing. The MiB figures are byte arithmetic (`width × height × 4`,
1 MiB = 1,048,576 bytes) or figures already published in the hotlist and Pass 46 audit, not
measured RSS. The orchestrator must build and run the affected crates' tests, and any future cut
must be accompanied by a scoped memory sample (`scripts/bench-memory`) at matched screen sizes.

PARTIAL — all four audit checks are resolved and both named cuts were evaluated, but neither is
safe without a measured CPU regression or a state that does not exist, so zero cuts were applied
and retained RAM is unchanged. The desktop already drops its decoded source and stores only the
output-sized raster; the lock's source-size copies are deliberate caches freed at process exit on
unlock, and the remaining ~9.6/25 MiB cropped-Fill staging cannot be removed with the existing
pixel-identical compositor because it uses a different scale filter. No build, test or runtime
sample was permitted in this unit, so the slice is UNBUILT and awaits orchestrator verification.
