# u78-release-profile

Scope: the `[profile.*]` sections of the root `Cargo.toml` only. Verification is static: every
claim below comes from `rg` over the workspace sources and from reading the manifests of the
binaries the profile sizes. No `cargo` or `git` command was run in this worker; the orchestrator
builds.

## Review of the shipped release profile

1. `panic = "abort"` — kept. `rg -n "catch_unwind|resume_unwind|AssertUnwindSafe|panic::catch"`
   over the workspace matches only `Cargo.toml` and prose documents, never a `.rs` file, so
   nothing in the tree needs unwinding to continue past a panic. Abort also drops the Rust
   exception-handling tables instead of shipping them for an unwind path no binary uses
   (smaller text, no landing pads on every call).

2. `lto = "fat"` vs `"thin"` — fat stays. Both are one build-time cost; fat additionally lets the
   linker fold the monomorphised widget/render instantiations that every binary duplicates across
   the `-ui-core`/`-ui-x11`/`-render-core` crates, so it is the smaller *and* faster artifact, not
   just the slower build. Thin only wins when the release build is run often; that need is already
   served by the separate `fast` profile (`lto = false`, `codegen-units = 16`).

3. `codegen-units = 1` — kept, for the same reason: whole-crate optimisation inside `-ui-x11` and
   `-render-x11` is worth more than incremental build reuse, which the `fast` profile owns.

4. `strip = "symbols"` — kept. Release already defaults to `debug = 0`, so there is no debuginfo
   for `strip = true` to remove as well; `"symbols"` is the whole remaining gain (no static symbol
   table in an installed, always-running desktop).

5. `opt-level = 3` for the workspace plus per-package `opt-level = "s"` — the split itself is
   correct and unchanged: per-frame work lives in `flamewm-render-x11`, `flamewm-ui-x11`,
   `flamewm-ui-core`, `flamewm-render-core` and `flamewm-image-core`, all left at 3; the resident
   app binaries that only drive widgets and events run at `"s"` so their text pages are fewer.

## Changes

1. `opt-level = "s"` extended to the two resident binaries that batch u50 missed:
   - `flamewm-task-manager` — its own 2.7k lines are `/proc` sampling and document building; the
     `Cargo.toml` dependency set is `flamewm-api`, `-control-dbus`, `-ui-core`, `-ui-document`,
     `-ui-x11`, i.e. exactly the shape already accepted for `flamewm-explorer`.
   - `flamewm-audio-helper` — 431 lines of `PulseAdapter` request plumbing that exits after 60 s
     idle; the pulse reads and snapshot encoding it forwards live in
     `flamewm-integrations-linux` at opt-level 3.
   Composition shift: two fewer always-resident text footprints per idle desktop (task manager
   open, WM respawning the helper), no change to a per-frame path.

## Left alone, with the reason

- `flamewm-terminal` stays at 3 even though it is resident: unlike explorer, it owns per-frame
  geometry in its own crate — the 69-rectangle box/block glyph fill in `glyphs.rs` and the
  run-composition of damaged rows in `app/grid.rs` run inside the terminal process on every
  output burst. Its blitting is in `-ui-x11`, but the crate-local part is large enough that
  `"s"` would trade input latency for a small page saving.
- `flamewm-screenshot` stays at 3: the whole selection overlay is its own code and is redrawn
  while the user drags the region.
- `flamewm-wm-wayland` stays at 3 (`colour_pass`, `colour_filter`, `cursor`, `scene`, `render/`
  are per-present work), as does `flamewm-x11-canary` (test tool path, not a shipped surface).
- `flamewm-ui` (156 lines) and `flamewm-apps` (76 lines) are too small for an override to change
  anything; `flamewm-apps` already has one and is harmless.
- `[profile.release.build-override] opt-level = 2` is build-time only; it emits no shipped code.
