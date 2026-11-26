# u122-binary-size

Area: resident binary text/RAM via Cargo feature sets and dependency weight (no build, read-only
Cargo.lock + manifest audit).

## Audit result (what is actually in the tree)

- `image`, `zbus`, `regex` are **not** dependencies of any workspace crate: no manifest line, no
  `Cargo.lock` package entry. Earlier slices already removed them, so there is nothing left to
  trim on those three; re-adding a comment in their place would only be churn.
- `smithay` (flamewm-wm-wayland only, so it never reaches the resident X11/shell binaries) is
  already `default-features = false` and every enabled feature has a call site: `backend_winit`
  (nested backend), `backend_egl` + `renderer_glow` (`EGLDisplay`/`GlowRenderer` in `drm/mod.rs`,
  `clip.rs`), `wayland_frontend` (reexported `wayland_server` dispatch in `lock_global.rs`),
  `desktop` (`PopupManager`, `layer_map_for_output` in `scene.rs`/`state/`), `xwayland`
  (`src/xwayland.rs`), `use_system_lib` (`ImportEgl::bind_wl_display`). Recorded next to the
  dependency so the next audit does not re-derive it.
- `resvg` keeps `raster-images` on purpose: the themed icon set ships SVG files with `<image>`
  elements (verified in `assets/themes/icons/MacTahoe`), and dropping the feature would silently
  render holes instead of failing. Feature parity, not fat. `text`/`system-fonts` stay off
  (`default-features = false`), so fontdb/ttf-parser are not linked.
- `zip` keeps `aes-crypto`: AES-encrypted archives are a reachable path in flamewm-archiver-app
  (password prompt → `extract_with_password`). ZipCrypto alone would not cover them.
- `dbus`, `chrono`, `serde`, `serde_json`, `rustix`, `inotify`, `bzip2`, `zstd` are already
  `default-features = false` (or minimal) and every enabled feature has a caller; `libpulse-binding`'s
  default feature set pulls only `libpulse-sys` (no `libpulse-simple-sys`/mainloop-glib in the lock).
- No unused direct dependency exists: scanning each crate's manifests against its `src/` found only
  `flamewm-ui-core` → `flamewm-ui-layout-core`, which is a deliberate `pub use ... as layout`.

## Change (owned paths: Cargo.toml files)

1. `Cargo.toml`: `x11rb` workspace entry dropped `randr`, `shape` and `xkb`. Nothing in the tree
   names `x11rb::protocol::xkb` (repo-wide `rg`; layout is xkbcommon's), so that protocol module
   was compiled for every X11 target and never called. `randr`/`shape` are now declared only by the
   crates that call them: `flamewm-wm-x11` (+`randr`,`shape`), `flamewm-x11-desktop` (`randr`),
   `flamewm-x11-canary` (`shape`); `flamewm-screenshot` already declared `randr` for itself.
2. `Cargo.toml`/crate manifests: comments pin the evidence for the `x11rb`, `smithay` and `resvg`
   feature decisions, so a later "trim the deps" pass does not remove a feature that is
   load-bearing (visual parity) or re-add one that has no caller.

Composition shift: the X11 binaries (`flamewm` WM, `flamewm-desktop`, shell/apps) no longer carry
the `xkb` protocol module in `x11rb-protocol`, and a build of the non-WM X11 crates no longer
compiles `randr`/`shape`. Text share is a small slice of these binaries — the large shares remain
`tiny-skia`+`resvg` (icons and wallpapers) and `fontdue`, both load-bearing — so the expected size
change is small per binary, and the larger effect is less compile work per crate graph. No visual
or feature change: every removed feature had zero call sites; every kept one is covered above.

Verification: manifest/Cargo.lock audit only (`rg`, `sed`, `wc`). Build and runtime checks belong to
the orchestrator's gate; this unit ran no cargo.
