# Unit u35 — Missing font and icon theme fallback audit

**Status: UNBUILT.** Static audit and source edits only. No `git`, `cargo`, build, test, Python or
sleep loop was run in this shared-checkout unit; the orchestrator builds, tests and commits after
the batch lands.

Route evidence: workspace version `0.1.7` was read directly from `Cargo.toml` (`scripts/version`
invokes Python, so it was not executed). Read `AGENTS.md`, `.opencode/HARNESS.md`,
`.opencode/ROUTES.md`, `.agents/README.md`, `.agents/CONTRACTS.md`, `.agents/JOBS.md`. Routed skills:
`flamewm-source-first` (current source, callers, fallible paths first) and `flamewm-rust-systems`
(Rust ownership and failure boundaries). Shared checkout: no `git`, no `cargo`, no branch, stash,
reset, rebase or commit operation.

## Audit scope and method

Searched `crates/**` for `fontdb`, `fontconfig`, `load_system_fonts`, `FontSystem`, `cosmic_text`,
`FontFamily`, `font_for`, `FontCache`, icon/cursor theme lookup (`resolve_icon_theme`, `IconTheme`,
`CursorTheme`, `index.theme`) and every `unwrap()` / `expect()` / `panic!` in those paths. `rg -l`
plus a five-line scan window covered multi-line chains; matches inside `#[cfg(test)]` modules,
`tests/` files, `*_tests.rs` and build scripts were separated from production code.

Excluded from edits by the brief: `crates/flamewm-integrations-linux/src/icon_service.rs` and the
shell icon files (`flamewm-shell/src/app/icons.rs` and the `async_projection/icon_*` files), which
other workers edit. They were audited read-only.

Findings:

- No `fontdb`, `FontSystem` or `load_system_fonts` call exists. Text comes from `fontdue` in
  `flamewm-render-core` and `flamewm-wm-wayland`, and from dynamically loaded Xft/fontconfig in
  `flamewm-render-x11`. `flamewm-image-core` builds `resvg` without the system-text stack.
- **Zero non-test `unwrap` / `expect` / `panic!` on a missing font, icon or theme remains in the
  audited crates.** The only keyword-adjacent `expect` calls were test fixtures, build-script
  staging of static assets, and the `CString::new("6x10" | "fixed")` invariants in
  `app/mod.rs:531,538` (static strings, cannot encode a missing font). No literal replacement was
  therefore required.
- No embedded font file (`.ttf`, `.otf`, `.woff*`) exists under `assets/` or `crates/`, so the
  "embedded font" branch of the fallback has nothing to use; the fallback is "skip text with a
  one-line failure record" on every path below.
- The gap was silence, not panics: the font fallbacks returned `None` / an estimate with no
  diagnostic. Following the u10 precedent, each silent missing-font site now emits one
  `flamewm:`-prefixed failure record. Records are guarded by `Once` where the fallback can recur,
  so a font-less system produces one line, not a line per glyph. With fonts present no record is
  emitted and no value, cache or draw path changes.

## Rows — file:line, failure, fallback, risk

| file:line | Failure | Fallback | Risk |
| --- | --- | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs:128,256-270` | fontconfig empty / none of the five candidate font files parseable → `find_font()` returns `None`, so `text_mask`, `advance_width` and `caret_positions` return `None`. | Already `Option`-based and cached as `Some(None)`; no `unwrap`/`expect` to replace. Added a one-line `NO_FONT_LOG` record when `find_font()` first fails. | Low — text is skipped exactly as before; one stderr line per process, no extra I/O on the hot path. No embedded font asset exists to promote. |
| `crates/flamewm-wm-wayland/src/font.rs:75-94,98-115` | No candidate font under `HOME`, `XDG_DATA_DIRS`, `/usr/share/fonts`, `/usr/local/share/fonts` → `load()` returns `None`. | `FontSlot::Missing` makes `with_glyphs` return `None` so the caller uses its built-in glyphs; no panic. Added a one-line `NO_FONT_LOG` record on the `Missing` transition. | Low — titlebar decoration still paints; title text uses the pre-existing built-in glyph path. `Missing` is cached, so the record fires once. |
| `crates/flamewm-render-x11/src/app/mod.rs:524-548` | `XLoadQueryFont` returns NULL for `6x10`, `6x13`, `9x15`, `10x20` and the `fixed` retry (no server fonts). | Core font map stays empty and `font_for_size` yields `None`; text is skipped. Added one `flamewm-render-x11: no core X font` record when the map is empty. | Low — runs once at app construction, independent of the Xft path; no change when any font loads. |
| `crates/flamewm-render-x11/src/xft/fonts.rs:119-142` | Xft/fontconfig init fails (library or usable font absent) for the process-wide measure. | `open_default_xft()` returned `None` and `default_text_measure` used `xft_measure_estimate`; kept, and added a one-line record with the error. | Low — estimate path unchanged; one record per thread that first needs a measure. No early-exit or panic. |
| `crates/flamewm-render-x11/src/xft/backend_measure.rs:57-100`; `backend.rs:91-95,148-179`; `app/mod.rs:800-823`; `external_drawable.rs:230-236` | No Xft family opens among preferred, IBM Plex Sans, Noto Sans, DejaVu Sans, sans-serif. | `font_for` returns `Err`, propagated by `?`; both `XftBackend::new` callers already log `FLAMEWM_RENDER_TEXT_BACKEND ... reason=` and fall back to the core path. No change. | None — already a logged fallback, not an unwrap. |
| `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:144,166`; `theme_roots.rs:36,54`; `themes/mod.rs:74,98` | Icon theme `index.theme` or KDE/GTK config missing/malformed → skip root, try GTK, then bundled default. | `ok()?` / `unwrap_or_default()` fallbacks unchanged; u10 already added one-line failure records. No change here. | None — read-only audit; the records and fallback order were shipped in u10. |
| `crates/flamewm-integrations-linux/src/icon_theme/index.rs:117-124`; `themes/cursor.rs:45` | No icon theme indexed, or `cursors/left_ptr` missing/oversized/malformed. | `hicolor` final fallback for icons; `Err(CursorError)` dropped by settings for the cursor preview. u10 record retained. No change. | None — graceful, already logged; no unwrap on a missing theme. |
| `crates/flamewm-shell/src/icon_loader.rs:288-323` (read-only) | Icon name misses every indexed theme and no bundled glyph applies. | `resolve_icon_raster` yields `None`; caller keeps the last valid raster or the transparent placeholder. All `expect` calls in this file are in `#[cfg(test)]` (from line 576). No edit — file owned by another worker. | Ownership risk only: this unit did not touch shell icon files. |

## Verification

Not run by this unit (see Status). The orchestrator should run the crate checks after the batch
lands; all four source edits are marked UNBUILT.

PARTIAL — the audit is complete: no non-test `unwrap`/`expect`/`panic!` on a missing font, icon or
cursor theme was found in the audited crates, so no literal replacement was needed; four silent
missing-font fallbacks now emit a one-line failure record with unchanged behavior whenever fonts
exist. Icon and cursor theme fallbacks were already hardened and logged in u10, and the excluded
shell/icon-service files were left untouched. Verification, build and commit belong to the
orchestrator, so this slice is UNBUILT and reported as PARTIAL rather than PASS.
