# u39-ui-doc2 changes

Batch five, area `ui-document` / `ui-layout-core` / `render-compiler`. Ranked by composition share:
`ui-document` runtime projection was already minimized in u12 (retained-layout memo, allocation-free
focus walk), `ui-layout-core` has no production consumer (evidence below), so the remaining slice in
this area is page compilation in `flamewm-render-compiler` (build time and peak build RAM).

Consumer evidence for `ui-layout-core`: the only reference outside the crate is
`crates/flamewm-ui/src/components/stack.rs` (`StackDirection`); `solve_stack`, `solve_grid`, `Grid`,
`LayoutItem`, `Insets`, `Measure` and `Alignment` have no caller in `crates/**`. Solver logic is left
alone (no composition share to win); one unused constructor is deleted.

## render-compiler (compile path)

- css: `strip_comments` returns `Cow<str>` and borrows the sheet when it has no `/*`. Composition
  shift: the whole stylesheet is no longer copied once per `parse_stylesheet` and once per
  `@import`-expanded file on every page compile.
- css: `split_top_level` returns `&str` slices instead of `String`s, so `parse_declarations` no
  longer allocates and copies one heap string per declaration before parsing it.
- css: `emit_rule_block` collects the expanded selector variants once and moves the parsed
  declarations into the last variant (`std::mem::take`). Composition shift: a single-selector rule
  block — nearly all of them — stops deep-cloning its declaration list (two `String`s per
  declaration) into the `Rule`.
- values: `parse_color_literal` compares keywords with `eq_ignore_ascii_case` and strips `rgb(` /
  `rgba(` case-insensitively instead of calling `trim().to_ascii_lowercase()`. Composition shift:
  this runs per colour declaration per element per state pass, so the removed allocation and copy
  was the function's dominant cost.
- html: `decode_entities` and `collapse_whitespace` return `Cow<str>` with a no-op fast path, and
  the text-node path no longer calls `trim().to_string()` on an already built `String`. Attribute
  values take one `into_owned()`. Composition shift: a text node or attribute without entities or
  whitespace runs (the common case) drops from three allocations to one; entities and collapsed
  text drop from three to two.
- html: `collect_style_text` appends into one `String` instead of `format!("{}\n", ...)` per
  `<style>` text child.
- rule_index: `candidates` extends a pre-sized `Vec` from the unkeyed bucket instead of cloning it,
  removing one allocation (and one more per element when no `*`-keyed rule exists).
- utilities: `rules_for` deduplicates candidates first and looks each distinct token up once,
  instead of `lookup` running once per occurrence plus once per distinct token.

## ui-layout-core

- deleted `Insets::symmetric`, which had no caller anywhere in the workspace.

Expected composition shift: lower build-time CPU and transient allocation volume while compiling
each `ui/` page and stylesheet; no change to compiled documents, styles, layouts or visuals.

## Not changed, on purpose

- `ui-document`: the retained-layout memo, `text_ref` borrow path, memoized focus walk and the
  appearance-token projections were already made allocation-lean in u12; `layout_box*` memo
  invalidation is covered by `continue_counters_from` bumping both revisions, so `replace_document`
  needs no manual memo reset.
- `ui-document/appearance_tokens`: `project_nav_row_tokens` builds a full `Appearance` for its
  `light` flag, but the only cheaper source (`glass::appearance_is_light`) is private to
  `flamewm-ui-core`; duplicating the appearance-key rule in `ui-document` would create a second
  owner for it, so it is left to that crate.

UNVERIFIED: no cargo/git in this shared checkout; the orchestrator runs the build, tests and commit.
