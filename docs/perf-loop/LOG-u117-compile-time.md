# u117-compile-time

Area: build hygiene without behaviour change — duplicate dependency versions, unused
dependencies/dev-dependencies, and default features in the workspace manifests.

- `flamewm-api` dev-dependency `toml`: `=0.8.23` (default features) -> `=1.2.3` with
  `default-features = false, features = ["parse", "serde", "std"]`. The only consumer is the
  `rules` test module and it calls `toml::from_str` only (`display`/`toml_writer` is never used).
  The 0.8 line was the sole reason Cargo.lock carried `toml_edit 0.22.27`, `toml_datetime 0.6.11`,
  `serde_spanned 0.6.9` and `winnow 0.7.15` alongside their 1.x twins; toml 1.x parses through
  `toml_parser`/`winnow` 1, which the build graph already contains via `proc-macro-crate`.
  Composition shift: the test build loses one `toml_edit` 0.22 copy plus three duplicated leaf
  crates; release binaries are untouched (dev-dependency).
- `flamewm-wm`: the inline `rustix = { version = "=1.1.4", ... }` declaration is replaced with
  `rustix = { workspace = true, features = ["fs", "process", "std"] }`. Same crate, same version;
  the workspace's `time` feature is already unified into every build of the crate, so no extra
  feature work is added, but the version can no longer drift from the workspace table.
- `flamewm-desktop`: inline `chrono` declaration (identical version and features to the workspace
  table) replaced with `chrono.workspace = true`, same reason.
- Audited every crate manifest for dead dependencies: every key in `[dependencies]`,
  `[dev-dependencies]` and `[build-dependencies]` has a non-comment use in that crate's `src/`,
  `tests/`, `examples/` or `build.rs`. The 250-line `dev-dependencies` set (x11rb `xtest` for the
  XDND tests, `flamewm-ui-x11` `test-support` for the shell/settings/explorer/desktop recorder
  tests, `flamewm-render-compiler` for the render-core integration tests, the wayland client
  crates for the wm-wayland examples) is all exercised, so none were dropped.
- Remaining Cargo.lock duplicates (`calloop` 0.13/0.14, `rustix` 0.38/1.1, `x11rb` 0.13/0.14,
  `thiserror` 1/2, `syn` 2/3, `bitflags` 1/2, `windows-sys` 0.48-0.61, `hashbrown` 0.15/0.17,
  `miniz_oxide` 0.8/0.9, `getrandom` 0.3/0.4) are semver-incompatible requirements of third-party
  crates (`winit`/`smithay-client-toolkit`/`drm`/`ndk` on the 0.13 line, `fontdue`'s `hashbrown`
  0.15, `ahash`'s `getrandom` 0.3, `png`'s `miniz_oxide` 0.8). No workspace manifest can unify
  those without changing a dependency version, so they were left as verified-not-ours.
- No cargo/git run in this unit (shared checkout); the orchestrator's next build refreshes
  Cargo.lock from the toml 1.2.3 bump.
