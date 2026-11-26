# u50-cargo-deps

Scope: workspace and crate `Cargo.toml` manifests only. Verification below is static (grep of
every `.rs` file in the owning crate, plus TOML parsing); the orchestrator runs the build, and no
`cargo` or `git` command was run in this worker.

## Review of the recent perf commits (manifests)

`git log -p -n 6 -- Cargo.toml 'crates/*/Cargo.toml'` showed only additive or subtractive
manifest edits, no regressions: batch five removed the already-unused
`flamewm-control-core`/`flamewm-dbus-reactor` edges from `flamewm-task-manager`, the window
management slice added `serde`/`toml` to `flamewm-api` and `rustix` to `flamewm-integrations-core`,
and settings added a `test-support` dev-dependency. The same unused-edge pattern still existed in
two other binaries, fixed here.

## Changes

1. Unused dependencies removed (each grepped across the whole crate including `tests/` and
   `benches/`; zero references, transitive availability unchanged because `flamewm-control-dbus`
   still depends on both):
   - `flamewm-assistant`: `flamewm-control-core`, `flamewm-dbus-reactor`.
   - `flamewm-imageviewer`: `flamewm-control-core`, `flamewm-dbus-reactor`.
   Composition shift: four fewer direct edges in the dependency graph and no unused extern
   entries; matches the `flamewm-task-manager` trim in batch five.

2. Shared dependency versions centralised in `[workspace.dependencies]` (`serde`, `signal-hook`,
   `wayland-client`, `wayland-protocols`, `wayland-protocols-wlr`) and members switched to
   `workspace = true` (`flamewm-api`, `flamewm-wm`, `flamewm-screenshot`, `flamewm-wm-wayland`,
   `flamewm-lock`). Feature sets are unchanged: per-member `features` merge with the workspace
   entry, and each entry reproduces the previous default feature set (`serde` keeps `std` through
   `default-features = false, features = ["std"]`, so the derive type stays identical).
   Composition shift: one version string per external crate, so the Wayland client/server and
   screenshot bindings cannot silently drift to two protocol versions in one binary.

3. Release profile: `opt-level = "s"` extended to the remaining resident app binaries whose own
   code is UI and event plumbing (`flamewm-apps`, `flamewm-archiver-app`, `flamewm-assistant`,
   `flamewm-explorer`, `flamewm-finder`, `flamewm-imageviewer`, `flamewm-lock`,
   `flamewm-settings`). Hot rendering and decoding stay in `flamewm-render-x11`, `flamewm-ui-x11`,
   `flamewm-render-core` and `flamewm-image-core` at `opt-level = 3`. Composition shift: fewer
   resident text pages per idle app, the same rationale already recorded for
   `flamewm-desktop`/`flamewm-shell`/`flamewm-wm`.

## Left alone

- `flamewm-wm`'s literal `rustix = "=1.1.4"` requests a strict subset of the workspace features
  (`fs`, `process`, `std`); inheriting the workspace entry would add `time` code, so the literal
  stays version-pinned.
- `fontdue` is used with defaults in `flamewm-render-core` (SIMD and hashbrown) and with
  `default-features = false` in `flamewm-wm-wayland`; one workspace entry cannot express both.
- The reported unused imports and dead functions in `flamewm-desktop`, `flamewm-explorer` and
  `flamewm-settings` live in `.rs` files outside this unit's owned paths. No unused dependency was
  found in those three crates, so the manifest side is clean and the source cleanup belongs to the
  units that own those files.
