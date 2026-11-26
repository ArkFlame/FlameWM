# u73-unwrap-audit-wm changes

Batch eleven, area `reliability: unwrap/expect/panic in desktop, reactor, render-x11`.
Scope: `rg` audit restricted to the three owned crates, with test modules excluded by
reading each file's `#[cfg(test)] mod` boundary, so only non-test code counts.

Composition reasoning (shares of the failure surface, not absolute CPU): these three crates
are the always-resident compositor + window-manager authority. A panic here is not a failed
widget: it takes the X connection, the managed windows and the desktop session with it. So the
whole reachable share of the idle and interactive path is exposed by one panic. Before this
unit every reachable site sat in the setup/paint path, which runs once per session start and
then per damaged frame; after it, no re-parsing of static names can abort the process.

Audit result:
- `flamewm-reactor`: every `unwrap`/`expect`/`panic!` is inside a `#[cfg(test)]` module
  (`lib.rs` from line 544, `child_pipe.rs` tests, `timeline.rs` tests). No production site.
- `flamewm-desktop`: the only production panics were the source-introspection helpers in
  `settings_sync.rs`; everything else (`wallpaper_loader`, `projection`, `downloads`,
  `sticky`, `rubber`, `wallpaper_fit`, widget tests) is test code. `build.rs` panics are
  compile-time only and are not a session path.
- `flamewm-render-x11`: sixteen production `expect` sites, all replaced. Numeric indexing in
  production was inspected: fixed-size channels/matrices (`[u8; 4]`, `[f32; 4]`, `data.l[..]`,
  union arms), length-guarded `as_bytes()[0]`, or `index < len` loops. No dynamic index panic
  found worth rewriting.

## flamewm-desktop/src/settings_sync.rs

- `source_between`, `signal_callback_source`, `settings_projection_source` are
  source-introspection helpers used only by contract tests (`workspace_sync` tests and this
  file's tests). They are now `#[cfg(test)]`, so `include_str!` + marker `panic!` no longer
  ship in the release build at all. Their panics remain, but as test failures, which is the
  intended signal for a drifted marker.

## flamewm-render-x11

- `ffi/dynamic_library.rs` `open_with_flags`: a candidate name containing an interior NUL is
  skipped and the next name is tried; if none loads, the existing `Err` is returned. An
  unusable library list is now a load error, not a session abort.
- `backdrop/canvas.rs` `load_canvas`: the two header words are read through
  `bytes.get(at..at+4)` returning `Option`; a short cache file falls back to `None`
  (recompute the backdrop) instead of panicking. The earlier length check already made this
  unreachable, so behaviour on valid files is unchanged.
- `external_drawable.rs` `external_content_hash`: `chunks_exact(8)` leftovers are handled with
  `<[u8; 8]>::try_from`; a short chunk contributes nothing instead of panicking. Only hit if
  the invariant broke, and then the hash is merely different.
- `external_decoration.rs` `retarget`: a missing session now returns
  `Err("external decoration has no live session")` instead of `expect`. Callers already
  receive `Result`, so the decoration is skipped and the rest of the session keeps drawing.
- `surface_controller/window_state.rs` `icon_geometry_atom` and
  `app/render/present.rs` `desktop_frame_atom`: a `CString::new` failure returns atom `0`.
  `XInternAtom`/`XDeleteProperty` with atom 0 is a no-op, so the caller behaves as if the
  atom were absent; the glyph/desktop frame keeps painting. The names are constants, so the
  fallback is unreachable in practice.
- `app/render/text_shadow.rs` `shadow_image`: the `xft.is_none()` guard already returned
  `Ok(None)`; the second borrow now uses `let Some(xft) = self.xft.as_mut() else { return
  Ok(None) }`, so a racing teardown of Xft yields "no shadow" instead of a panic.
- `xdnd.rs` `selection_bytes` ArkPath arm: if the session was already taken, it returns
  `self.fail(source)` (rejected `Finished` reply, no drop event) instead of `expect`.
- `app/render/paint_full.rs` `retained_command`: now returns
  `Option<*const PaintCommand>`, `None` when no paint cache is installed. The three callers
  (`paint_full`, `paint_region`, `text_shadow::prefetch_text_shadow_masks`) skip that command
  with `continue`, which is what an absent cache means: nothing retained to replay. All three
  callers already checked `cached_paint` before the loop, so valid frames are unchanged.
- `app/images.rs` cache eviction: `remove(&eviction_key)` after `keys().next()` now breaks the
  eviction loop when the key is gone (impossible today), leaving the insert to respect the
  remaining budget check on the next frame.
- `accel/gles.rs` `shader`: an interior NUL returns `Err("shader source contains an interior
  NUL")`, which the existing shader-build error path already propagates (GPU path falls back).
- `accel/gles/loader.rs` `extension_proc`: an interior NUL returns `null_mut()`, the same
  result `eglGetProcAddress` gives for an unknown name.

Expected composition shift: crash-propensity on the always-resident WM/compositor path drops
to zero for these sites; no idle wakeups added, no extra allocation on the hot path (the
`Option` return of `retained_command` is a pointer-width value computed exactly as before),
and static name parsing only becomes fallible on inputs that were process-fatal before.

UNVERIFIED: no cargo/git in this shared checkout. `rustfmt --edition 2024 --check` on all
fourteen edited files exits 0. The orchestrator runs the build, tests and commit.
