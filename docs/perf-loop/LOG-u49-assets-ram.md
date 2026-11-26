# u49-assets-ram log

Batch six over assets/RAM in the owned paths: `crates/flamewm-skin`, `crates/flamewm-ui` and
`crates/flamewm-shell/src/icon_loader.rs`. `git` and `cargo` are forbidden in this shared checkout, so the
review of the earlier slices is source-based and the orchestrator runs the batch gate. Review findings first,
then the new slices.

## Review of the landed slices (no commit log available)

The palette work from u23 (memoised `ink_palette_on`/`on_accent`, sRGB-to-linear LUT) and the icon-loader
work from u36 (borrowed pending probe, in-place pending removal, `coalesced` metric) are sound and stay. The
source read surfaced two defects in `icon_loader`, both fixed here:

- `drain_ready` removed the pending key *before* the stale-generation check. After a theme switch the worker
  can still answer requests issued under the old generation; those stale replies then deleted the key that
  the view had just re-inserted for the new generation. The new request stopped coalescing and the icon was
  resolved twice. Fixed by checking the generation first: only a reply of the current generation may release
  a key, and the key is released after the check.
- `bump_generation` advanced the counter but left `pending`/`retry` to the caller. `set_theme` cleared them,
  but any other generation bump (or a future caller) would strand keys whose replies are now stale. The clear
  moved into `bump_generation`, so "a pending key always has a live reply" is now an invariant of the type;
  `bump_generation` is crate-private (no external caller).

## New slices

- **icon_loader: idle drain fast path (`drain_ready`).** Every shell turn called `drain_ready`, which did a
  wake-pipe `read`, a theme/retry flush and a `try_recv` probe even when every visible icon was already
  applied. A new `replies_outstanding` counter is incremented on each successful worker send and decremented
  per received reply (any generation); the drain now returns immediately when no key is pending, no retry or
  theme is queued and no reply is in flight. Composition shift: on an idle or settled desktop the icon path
  costs no syscall and no channel probe per turn; in-flight requests and stale post-theme replies still take
  the full path (the counter keeps the gate open until they drain), so behaviour and latency are unchanged.
  `TryRecvError::Disconnected` zeroes the counter so a dead worker cannot keep the path awake.
- **icon_loader: cheap pending-map hash (`PendingSet`).** The coalescing map is keyed by the five-variant
  `IconTarget`; the default SipHash cost more than the single-entry compare it guards. It now uses a local
  FNV-1a `BuildHasherDefault`, so each request probe is one byte mix instead of a SipHash round. The map
  contents, coalescing semantics and counters are unchanged.
- **icon_loader: batched wake drain.** `wake_drain` now reads 256 bytes per syscall instead of 64, so a burst
  of one-byte worker writes (startup, dock/app list change) is consumed in fewer reads.
- **skin/palette: one luminance per scrim search (`scrim_alpha_floor_with`).** The binary search recomputed
  `relative_luminance(ink)` on every probe even though the ink is fixed; it is now solved once and the loop
  uses a new private `contrast_from_luminance`. Two floors run per `glass_ink`, so a glass solve drops about
  16 redundant luminance evaluations; `contrast_ratio`, `on_accent_uncached` and `ink_palette_on_uncached`
  share the helper (the accent and surface solves stop computing the same luminance twice). Results are
  bit-identical; the cache keys and caps are untouched.
- **flamewm-ui: Toggle state change without a round trip.** `Toggle::set_checked` built a `ToggleState`,
  replayed the old value into it and read the new one back for a two-boolean comparison. It compares and
  assigns directly, dropping the dependency hop and the stack copy per click.

## Not done here (out of the owned paths)

- The unused-import/dead-function warnings named for `flamewm-desktop`, `flamewm-explorer` and
  `flamewm-settings` are outside this unit's paths (`crates/flamewm-skin`, `crates/flamewm-ui`,
  `crates/flamewm-shell/src/icon_loader.rs`); they stay for their owning units.
- The low-priority counters `low_enqueued`/`dropped_low` were already removed from the loader (and its
  consumer) by a concurrent slice while this unit ran; no dead field remains in the owned file.
- `PendingSet::len` is test-only and is now `#[cfg(test)]`, so non-test builds do not carry it.

Checks: no `cargo` and no `git` were run (shared checkout, orchestrator runs the batch gate). Compilation
correctness argued by reading: `BuildHasherDefault<TargetHasher>` satisfies `HashMap`'s `S::Default` bound and
`TargetHasher` implements the `Hasher` methods the derived `Hash` for the fieldless enum calls; the new
counter is initialised in both `IconLoader` literals (spawn and the test helper) and the one test that injects
replies without `request` sets it explicitly; `bump_generation` is only referenced from `set_theme` and the
in-file tests; `contrast_from_luminance` and the colour edit keep the public signatures unchanged.
