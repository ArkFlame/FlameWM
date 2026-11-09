# UI utilities spec: Tailwind-familiar names, used-only, compile-time

Goal: short, widely known class names (`flex items-center gap-2 p-3 rounded-lg bg-accent`) for the patterns we
actually repeat, with no new language, no build tool, no runtime cost, and no cost for anything not used.

## Evidence (ui/**/*.css, 17 sheets)
5654 declarations, 1057 unique. Top repeats: `display:flex` 444, `align-items:center` 290, `width:100%` 159,
`flex-direction:column` 155, `position:absolute` 139, `justify-content:center` 117, `overflow:hidden` 107,
`white-space:nowrap` 104, `flex-grow:1` 98, `color:var(--text)` 69, `color:var(--muted)` 64, `gap`/`border-radius`/`font-size`
with token values. Hardcoded: 328 hex literals and 2150 px literals outside tokens.css.

## Architecture (3 rules)
1. **Static table, no parser of its own.** `crates/flamewm-render-compiler/src/utilities.rs` holds one sorted
   `const` array `(name, &[(property, value)])` plus a small scale function for numeric steps. Lookup is binary
   search on the class token. Anything not in the table is an ordinary component class and is ignored.
2. **Used-only (tree-shaken at compile time).** While compiling a document the compiler already walks every
   `class` attribute. Each distinct token is looked up once; only hits become synthetic single-class rules,
   deduplicated per document, appended after all linked sheets (same specificity as a class rule, so a utility
   overrides a component class, as in Tailwind). A page with no utility classes does zero extra work beyond the
   token lookup and emits nothing. The compiled `.rwr` grows only by the rules that page uses. No runtime
   parser, no runtime cost.
3. **Values come from tokens, never invented.** Spacing steps `1..12` = 4px multiples (`p-3` = 12px) and token
   aliases (`p-md` = `--space-md`); radius `rounded-sm|md|lg|xl` = `--radius-*`; type `text-xs|sm|base|md|lg|xl` =
   `--font-*`; colours `bg-accent|accent-soft`, `text-text|muted`, `border-rim` = `var(--name)` for tokens that
   exist. Arbitrary values `p-[10px]`, `w-[240px]` accept only what `parse_length` already accepts (px, %, var()).
   Unknown value = ignored token + a compile-time warning list in tests, never a panic.

## Tailwind compatibility (the language is Tailwind's, not ours)
Names, argument shapes and semantics follow Tailwind CSS v3/v4 exactly, so a model or person writing Tailwind gets
the expected result:
- Spacing scale = Tailwind's: 1 step = 0.25rem = 4px (`p-4` = 16px, `gap-2` = 8px, `w-12` = 48px, `p-0.5` = 2px,
  `px-1.5` = 6px), plus `px` (1px), `full` (100%), `auto`, fractions `w-1/2` (50%), `w-1/3`, `w-2/3`, `w-1/4`.
- Arbitrary values use Tailwind's bracket form: `w-[240px]`, `p-[10px]`, `gap-[var(--space-md)]` (px, %, var() only).
- Same names for flex/grid-less layout, `items-*`, `justify-*`, `flex-1`, `grow`, `shrink-0`, `truncate`, `overflow-*`,
  `font-medium|semibold|bold`, `rounded`, `rounded-sm|md|lg|xl|full`, `border`, `border-2`, `opacity-N`,
  `cursor-*`, `select-none`, `absolute inset-0`.
- Named scales that Tailwind defines in rem map to our tokens because the desktop type/radius scale is smaller:
  `text-xs|sm|base|lg|xl` -> `--font-xs|sm|base|lg|xl` (our px sizes, not Tailwind's), `rounded-sm|md|lg|xl` -> `--radius-*`.
  This is the only deviation and is stated in the generated docs table.
- Colours: Tailwind form `bg-<token>`, `text-<token>`, `border-<token>` over our semantic tokens (`accent`,
  `accent-soft`, `text`, `muted`, `rim`, `transparent`, `white`, `black`). The Tailwind default palette (`blue-500`) is
  NOT shipped (it would hurt theming and size); a new colour is a token first.
- Variants use Tailwind syntax `hover:bg-accent-soft`, `focus:`, `active:`, `disabled:` where the compiler supports the
  pseudo-class; anything else is rejected at compile time in tests, not silently dropped.
- Not supported (documented, so nobody guesses): `@apply`, `@theme`, arbitrary variants, `dark:`/responsive variants,
  opacity slash modifiers (`bg-accent/50`) in v1.

## v1 table (about 90 names; extend only from measured repeats)
- layout: `flex inline-flex block hidden relative absolute`, `flex-row flex-col`, `flex-1 grow grow-0 shrink-0`,
  `items-start|center|end|stretch`, `justify-start|center|end|between`, `min-w-0`, `overflow-hidden`, `box-border`
- sizing/spacing: `w-full h-full w-N h-N w-[..] h-[..]`, `p|px|py|pt|pr|pb|pl-N`, `m|mx|my|mt|mr|mb|ml-N`, `gap-N`,
  `inset-0 top-0 left-0 right-0 bottom-0`
- text: `text-xs..xl`, `font-medium|semibold|bold`, `truncate` (nowrap+hidden+ellipsis), `whitespace-nowrap`,
  `text-text text-muted text-accent`
- surface: `bg-accent bg-accent-soft bg-transparent`, `border border-rim`, `rounded-sm..xl`, `opacity-N`
- interaction: `cursor-default cursor-pointer select-none`
- variants: only `hover: focus: active: disabled:` and only if the compiler already supports that pseudo-class;
  no responsive, no dark (dark is token swapping), no `@apply`, no `@theme`, no opacity modifiers in v1.

## Performance budgets (each enforced by a test or measured number)
- Page without utility classes: compile time delta <= 1% (median of 200 compiles).
- Page with utilities: rules emitted <= number of distinct utility tokens used; nothing else.
- Table lookup allocation-free (`&'static` data); no regex, no string formatting on the hot path.
- Zero change to layout/paint crates: utilities lower to the declarations the compiler already understands.

## Safety
- Collision test: no existing CSS selector may define a class whose name equals a utility name (test fails with
  the list), so existing pages cannot change by accident.
- Equivalence test: a fixture written with utilities and the same fixture with hand-written CSS compile to
  identical layout rects.
- Docs table is generated from the const table in a test (`docs/UI_UTILITIES_SPEC.md` names == table names).

## Rollout
1. Compiler core + tests (new file, one hook where class attributes are resolved).
2. Collision and coverage audit on all 17 sheets, final v1 name list from measured counts.
3. Pilot: migrate one shell surface and one settings section; report CSS bytes, `.rwr` bytes, compile ms before/after.
4. Only then migrate further, by measured duplicate count; never mass-rewrite.
