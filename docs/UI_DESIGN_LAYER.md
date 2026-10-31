# UI Design Layer

Status: steps 1 and 2 partly done. Scope: `ui/**/*.css|html`, compiled by `flamewm-render-compiler`.

## Goals

- Define surfaces, outlines, radii, accent/surface colours, buttons, cards, lists, menus and scrollbars once.
- Every app links the same sheets; app sheets hold only app-specific layout.
- Zero runtime cost: all sharing resolves at compile time. The compiled `.rwr` tree already holds resolved
  per-node styles, so a shared rule adds no runtime memory.
- No new compiler surface unless a measured duplicate needs it.

## Survey (measured)

Compiler facts (`crates/flamewm-render-compiler/src`):

- No at-rules. `css.rs:70` rejects every `@`, so `@import`/`@use` are unavailable.
- Compile-time include already exists. `compile.rs` `compile_file` concatenates every local
  `<link rel="stylesheet">` in document order, then any `extra_css_paths`. Relative `href`s resolve from
  the HTML file's directory, so `../base/base.css` works. Network/data URLs are rejected.
- `var()` works for colours only (`parse_color_value`, `compile.rs:703`). Lengths, radii and fonts cannot
  reference tokens.
- `:root { --name: <literal colour> }` is collected across the whole combined sheet into one table
  (`collect_variables`, `compile.rs:122`). A later definition of the same name wins.
- Runtime colour overrides already use this table: `flamewm-settings/src/nodes.rs:111`
  (`document.color_variable`) and `flamewm-desktop/src/settings_values.rs:51` (`--accent`, `--accent-soft`,
  `--accent-strong`). The skin/accent path needs no new mechanism.
- Unsupported (per request): `calc()`, `letter-spacing`, `text-align`, `margin:auto`, `border:none`,
  per-corner radius.

Duplication in `ui/` (13 CSS files, 1,279 lines, excluding `assets`):

- `ui/flamewm.css` (showcase, 209 lines) vs `ui/shell/flamewm.css` (shell, 271 lines): 133 of 217
  rule blocks in the showcase are identical to shell rules (61%).
- Reset `* { box-sizing:border-box }` appears in 8 files. `html { width:100%; height:100%; }` appears in 10.
- 199 distinct declarations appear in 3 or more files.
- Token drift: `--muted` is `#d2d2d8` in `assistant.css` but `#a1a1a6` in `osd.css`. Tokens are not yet one set.
- Guards with hard-coded file lists: `tools/ui_contract_guard.py`, `tools/accent_default_guard.py`.
  A new shared sheet that defines colours or the accent must be added there (not yet done).

Reproduce the counts: `python3 -I` over `ui/**/*.css` with a block regex (scratch scripts, not committed).

## Architecture (minimal)

```text
ui/base/base.css         reset + colour tokens (:root) shared by all apps    (step 1: reset only)
ui/shared/menu.css       menu rows (already shared by task-manager and shell)
ui/<app>/<app>.css       app layout only; may override a token in its own :root (last definition wins)
ui/<app>/<app>.html      <link> base first, then shared components, then app sheet
```

- Sharing is by `<link>` order. The compiler concatenates in order, so a later sheet overrides an earlier one.
- Utility classes (Tailwind-style generator) are not planned. They would add a build tool and rule volume
  without a measured duplicate that needs them. Revisit only if the component sheets miss a repeated
  pattern.
- Length/radius tokens need a compiler change (`parse_length` accepting `var()`). Deferred until a
  measured need exists. Colour tokens need no compiler change.

## Migration plan (ordered, small steps)

1. Done in this change: `ui/base/base.css` holds the shared reset. `ui/assistant/assistant.html` links it
   first. The reset is removed from `assistant.css`. `flamewm-assistant/build.rs` tracks the base file.
   Verified: `cargo test -p flamewm-render-compiler -p flamewm-assistant` exit 0 (includes
   `linked_shared_stylesheet_matches_inline_sheet`). Compiled `assistant.html` before (HEAD sources)
   and after this change match byte-for-byte, except the 8-byte `source_fingerprint` at offset 8. That
   field is an FNV hash of the HTML and CSS source text, so it changes by construction.
   Not verified: Xephyr screenshot; guard scripts.
2. Partly done. `--rim` (identical in every sheet that defines it) and the reset now live in `base.css`. Settings,
   finder, explorer, terminal and imageviewer link it first. Verified: their compiled `.rwr` match HEAD byte-for-byte
   apart from the fingerprint (assistant gains one unused variable entry, +13 bytes). Not moved yet: `--accent`,
   `--text`, `--muted`, `--soft`, ... differ per app (`--muted` drifts), so moving them changes output. Resolve the
   drift per app before merging, because last-wins changes the colour silently.
3. Add `ui/base/components.css`: button reset, scrollbar, list row, card. Migrate `assistant` first, then `osd`.
4. Blocked: keep `ui/index.html` and `ui/flamewm.css`. Decision: the showcase is referenced outside docs, so
   it is not deleted. Referrers (`git grep`, at origin/main):
   - `scripts/check:20` compiles `ui/index.html`.
   - `tools/ui_contract_guard.py:27,151` and `tools/accent_default_guard.py:27` list `ui/flamewm.css`.
   - `tools/test_ui_contract_guard.py:27-28` writes both files as fixtures.
   Dedupe is not byte-equivalent. Linking `ui/shell/flamewm.css` adds 130 shell-only blocks. A residual
   `ui/flamewm.css` (132 shared blocks removed) gives 19 variables vs 18, +20 bytes, and about 2.2 MB of
   differing bytes, because the cascade order changes. Revisit only after the referrers are migrated.
5. Migrate `task-manager`, `finder`, `explorer`, `terminal`, `imageviewer`, `lock`, `settings`, `desktop`.
6. Teach `tools/ui_contract_guard.py` and `tools/accent_default_guard.py` about `ui/base/*.css`.
7. Only if measured: length/radius `var()` in `flamewm-render-compiler`, with its own tests.

## Savings estimate

- Assistant (step 1): 60 to 58 lines in `assistant.css`. The reset now lives once in a 2-line base file.
- Full migration: the reset repeats about 8 to 10 times. Showcase vs shell dedupe removes up to 133 blocks
  (about 200 lines of the 209-line showcase) if the showcase links the shell sheet. Other savings come from
  component sheets. Expect roughly a third of app-sheet lines to move into shared sheets. This is an
  estimate from the duplicate counts, not a measured result.

## Risks

- Silent override: a `:root` defined in a later sheet overrides a base token for that app only. Keep one
  owner per token, and check the drift before merging.
- Runtime accent: `settings` and `desktop` override variables by name. Renaming a token breaks the
  override path. Keep names stable.
- Showcase: `ui/index.html` and `ui/flamewm.css` may be treated as product UI by mistake. Do not edit them
  as part of the layer until step 4.
- Guards hard-code file lists. A missed guard update would let a shared colour drift unnoticed.
- Packaging: the base file is compiled at build time, so it needs no runtime file. Not verified against
  packaging scripts.
