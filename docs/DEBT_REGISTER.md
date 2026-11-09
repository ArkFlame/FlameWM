# Debt Register: image-core, render-core, window-core, api, platform

Snapshot: `origin/main` at `20449c57`, workspace version `0.1.7` (`scripts/version`).
Scope: `flamewm-image-core`, `flamewm-render-core`, `flamewm-window-core`, `flamewm-api`, `flamewm-platform`.
`render-x11`, `wm-x11`, `ui-core`, `explorer` and `terminal` belong to other workers; items that
need them are listed as deferred.

"Non-test lines" means production lines: `#[cfg(test)]` items and `//` comments are not counted.
That matches `source_loc` in `tools/architecture_guard.py`.

| # | item | path:line | type | size | fix | risk |
|---|---|---|---|---|---|---|
| 1 | Oversized model file | `crates/flamewm-render-core/src/model/style.rs:1` | file size | 704 total, 693 non-test | split types (`:1-289`) and the `RuntimeDocument` impls (`:290-704`) into `style/mod.rs` + `style/runtime.rs`; `pub use style::*` unchanged | low |
| 2 | Oversized codec with inline tests | `crates/flamewm-render-core/src/codec.rs:1` | file size + inline tests | 996 total, 631 non-test; tests `:631-996` | tests to `codec/tests.rs`; `Wire` table, `Writer`, `Reader` to `codec/{wire,writer,reader}.rs`; `encode`/`decode` stay | low (wire bytes covered by round-trip tests) |
| 3 | Over the 600 line limit by 2 | `crates/flamewm-platform/src/shortcuts.rs:562-600` | file size | 602 non-test | move `is_bare_typing_key`, `validate_binding`, `is_modifier_only` to `shortcuts/validation.rs` | low (next step) |
| 4 | Duplicate helper: rounded-corner clamp | `crates/flamewm-render-core/src/paint.rs:346` = `crates/flamewm-render-x11/src/native/rounded.rs:5` | duplicated helper | 3 lines, same formula | keep render-core owner; render-x11 imports it | low; deferred (render-x11 owned elsewhere) |
| 5 | Duplicate body: backdrop clip | `crates/flamewm-render-core/src/lower.rs:202` = `crates/flamewm-render-x11/src/app/surface_backdrop.rs:351` | duplicated body | identical after whitespace normalisation | render-core owner, pub fn, render-x11 imports | medium (new cross-crate pub item); deferred |
| 6 | Duplicate body: hole row span | `crates/flamewm-render-core/src/soft.rs:87` = `crates/flamewm-render-x11/src/accel/mod.rs:104` | duplicated body | identical after whitespace normalisation | render-core owner, pub fn, render-x11 imports | medium; deferred |
| 7 | Not a dedupe: `Rect::clamp_inside` | `crates/flamewm-api/src/geometry.rs:124` vs `crates/flamewm-wm-x11/src/geometry.rs:29` | near-duplicate, divergent semantics | 14 lines each | keep both. API returns `default()` for invalid bounds and wm-x11 uses `max(1)` and `saturating_sub`; merging changes behaviour | n/a |
| 8 | Facade LOC ratchet | `docs/ARCHITECTURE_EXCEPTIONS.toml` (`crates/flamewm-platform/Cargo.toml`, `facade-source-loc>5000`, `remove_by 0.3.0`) | architecture exception | 6406 production lines in one facade | move policy into owning `*-core` crates; the exception must be removed in the same change that drops below 5000 | high |
| 9 | Stale exceptions | `docs/ARCHITECTURE_EXCEPTIONS.toml` | exception ratchet | 76 entries (18 `remove_by 0.2.0`, 58 `0.3.0`) | none to prune: the guard fails on any stale entry and passes now (`76 ratified violations`) | none |
| 10 | Dead code | in-scope `pub` items | dead code | 0 pub items with no reference outside their own definition line; 0 `allow(dead_code|unused)` | none | none |
| 11 | Unowned TODO/FIXME | in-scope `*.rs` | stale marker | 0 | none. Owner-tagged `TODO(J12-PENDING)` markers in `render-core/src/model/document.rs` and `model/style.rs` are guard-tracked | none |
| 12 | Test-heavy file, not oversize | `crates/flamewm-render-core/src/pixels.rs:484` | inline tests | 970 total, 484 non-test | none. Optional: move tests to `pixels/tests.rs` for readability | none |

## Measurements

- Production line count: awk over each file, stopping at the first `#[cfg(test)]` line. This is exact when tests sit at the end of a file, which holds for items 2, 3 and 12. Item 1 has a small `length_tests` module at `style.rs:48`, so its figure comes from `source_loc`-style counting (693).
- Dead-code scan: every `pub fn|struct|enum|type|trait|const|static` in scope, checked for any identifier token in another `.rs` file of the workspace. 139 items are referenced only in their own file. These are internal uses, not dead code. The `dead_code` lint already covers unused private items.
- Duplicate scan: normalised function bodies (whitespace collapsed) grouped across crates. In scope the cross-crate groups are items 5 and 6, both with render-x11.
- Names in the original brief were stale: `image-core/src/blur.rs` is 237 lines (not about 1468), and `render-core/src/layout.rs` is 396 lines (not about 1458).

## Execution plan

1. Items 1 and 2: split. One gate per commit.
2. Item 3: split. Next session.
3. Items 4 to 6: deferred until render-x11 is free.
4. Item 8: needs a plan from the platform owner.
