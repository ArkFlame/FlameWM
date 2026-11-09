# UI utilities audit (v1 measurement)

Status: measurement only. No CSS, Rust or settings code changed. Base: `origin/main` 9a0dae1b.
Governing spec: `docs/UI_UTILITIES_SPEC.md` (v1 table). Related: `docs/DESIGN_TOKENS.md`, `docs/UI_DESIGN_LAYER.md`.

## Method and caveats

- Scripts (not committed): `/tmp/claude-1000/uiaudit/{parse,audit,analysis,totals,pilot}.py`, run with `python3 -I <script> <repo>`.
- Scope: `ui/**/*.css` only, 17 sheets, 1197 rule blocks, 5669 declarations, 1020 unique (normalised).
  The spec quoted 5654 / 1057; the difference is parser counting (`:root` and empty-rule handling), not a data change.
- Matching: whitespace-normalised, lowercase; `var(--token)` resolved per sheet (`ui/base/*.css` first, then the sheet's own `:root`).
  `0` matches `0px`. Multi-declaration names match when every declaration appears in one rule.
- Bytes = normalised `prop:value;` text. Gross, not net: utility class tokens added to HTML cost bytes too (not counted).
- Token drift is real and changes matches: `--text`, `--muted`, `--rim`, `--accent`, `--accent-soft` have 2-6 values
  across sheets (for example `--muted` = `#d2d2d8` / `#a1a1a6` / `#8a8a8a`). Per-sheet resolution is used; drift must be fixed before the utilities freeze colours.

## 1. v1 names: replaced declarations (rules / gross bytes)

Coverage: 3780 of 5669 declarations (66.7 %, 59200 of 90435 bytes, 65.5 %) match a v1 name or scale step.
`rules` = rule blocks containing the full declaration set. `border` and `border-rim` are the same declaration (duplicate; keep one).

Layout, sizing, position:
flex 455 rules / 5915 B; items-center 301 / 5719; flex-col 155 / 3410; justify-center 126 / 2898; absolute 144 / 2592;
w-full 161 / 1771; h-full 63 / 756; overflow-hidden 108 / 1728; min-w-0 44 / 616; box-border 30 / 660;
grow 98 / 1176; grow-0 84 / 1008; flex-row 80 / 1520; relative 28 / 504; block 13 / 182; hidden 2 / 26;
items-start 4 / 92; items-end 5 / 105; items-stretch 5 / 100; justify-start 15 / 405; justify-end 21 / 525; justify-between 23 / 690;
inset-0 4 / 152; top-0 66 / 528; left-0 72 / 648; right-0 7 / 70; bottom-0 11 / 121.

Text, font, colour:
whitespace-nowrap 113 / 2147; truncate 17 / 986; text-xs 19 / 285; text-sm 40 / 600; text-base 48 / 720; text-md 46 / 690 (not in spec);
text-lg 27 / 405; text-xl 15 / 225; font-semibold 34 / 544; font-bold 28 / 448; font-medium 18 / 288;
text-text 84 / 1512; text-muted 76 / 1064; text-accent 13 / 182; bg-transparent 69 / 1587; bg-accent 47 / 893; bg-accent-soft 22 / 660;
border (= border-rim) 86 / 2408; cursor-default 28 / 420; cursor-pointer 8 / 120.

Surface:
rounded-sm 35 / 630; rounded-md 34 / 612; rounded-lg 33 / 594; rounded-xl 29 / 551; rounded-full 5 / 100.

Scale families (spec steps, 4 px per step, half steps included):
h-N 260 rules / 3082 B; w-N 159 / 1697 B; gap-N 176 / 1482 B; p-N 63 / 782 B; pl-N 22 / 387 B; m-N 16 / 176 B; other sides < 10 each.
Top steps: `h-7.5` 44, `w-0` 38, `gap-2` 35, `gap-2.5` 34, `p-0` 25, `h-7` 24, `gap-1.5` 23, `gap-0.5` 20, `h-9` 20, `gap-3` 19.

Zero matches: `inline-flex`, `flex-1` (`flex:1` appears twice; spec maps it to `flex:1 1 0%`), `shrink-0`, `select-none`, `opacity-N` (max 2 per value).

## 2. Top 40 multi-declaration groups (rule-level, 2-4 declarations, all mapped to v1)

Columns: score (rules x bytes) | rules | bytes per rule | v1 composition | declarations. Rows are sub-sets, so one rule can count in several rows.

9216 288 32 | items-center flex | align-items:center ; display:flex
6270 114 55 | items-center flex justify-center | align-items:center ; display:flex ; justify-content:center
4872 116 42 | items-center justify-center | align-items:center ; justify-content:center
4760 136 35 | flex flex-col | display:flex ; flex-direction:column
4284 119 36 | flex justify-center | display:flex ; justify-content:center
3111 61 51 | items-center flex flex-row | align-items:center ; display:flex ; flex-direction:row
2560 80 32 | flex flex-row | display:flex ; flex-direction:row
2419 59 41 | border flex | border:1px solid var(--rim) ; display:flex
2376 99 24 | flex w-full | display:flex ; width:100%
2340 39 60 | items-center border flex | align-items:center ; border:1px solid var(--rim) ; display:flex
2318 61 38 | items-center flex-row | align-items:center ; flex-direction:row
2107 49 43 | items-center flex w-full | align-items:center ; display:flex ; width:100%
1995 57 35 | left-0 absolute top-0 | left:0px ; position:absolute ; top:0px
1890 54 35 | overflow-hidden whitespace-nowrap | overflow:hidden ; white-space:nowrap
1833 39 47 | items-center border | align-items:center ; border:1px solid var(--rim)
1755 65 27 | left-0 absolute | left:0px ; position:absolute
1672 38 44 | items-center flex h-7.5 | align-items:center ; display:flex ; height:30px
1656 36 46 | flex flex-col w-full | display:flex ; flex-direction:column ; width:100%
1650 30 55 | items-center bg-transparent flex | align-items:center ; background:transparent ; display:flex
1632 32 51 | items-center flex whitespace-nowrap | align-items:center ; display:flex ; white-space:nowrap
1586 61 26 | absolute top-0 | position:absolute ; top:0px
1566 29 54 | items-center flex flex-col | align-items:center ; display:flex ; flex-direction:column
1541 23 67 | items-center flex h-7.5 justify-center | align-items:center ; display:flex ; height:30px ; justify-content:center
1525 61 25 | flex grow | display:flex ; flex-grow:1
1504 32 47 | flex flex-col grow | display:flex ; flex-direction:column ; flex-grow:1
1488 24 62 | items-center flex flex-row w-full | align-items:center ; display:flex ; flex-direction:row ; width:100%
1470 49 30 | items-center w-full | align-items:center ; width:100%
1411 17 83 | items-center border flex justify-center | align-items:center ; border:1px solid var(--rim) ; display:flex ; justify-content:center
1395 45 31 | flex absolute | display:flex ; position:absolute
1386 22 63 | items-center flex flex-row grow-0 | align-items:center ; display:flex ; flex-direction:row ; flex-grow:0
1376 32 43 | flex flex-row w-full | display:flex ; flex-direction:row ; width:100%
1363 47 29 | flex overflow-hidden | display:flex ; overflow:hidden
1332 18 74 | items-center flex justify-center whitespace-nowrap | align-items:center ; display:flex ; justify-content:center ; white-space:nowrap
1323 21 63 | border flex flex-col | border:1px solid var(--rim) ; display:flex ; flex-direction:column
1260 30 42 | items-center bg-transparent | align-items:center ; background:transparent
1254 38 33 | flex-col w-full | flex-direction:column ; width:100%
1248 16 78 | items-center bg-transparent flex justify-center | align-items:center ; background:transparent ; display:flex ; justify-content:center
1242 23 54 | items-center h-7.5 justify-center | align-items:center ; height:30px ; justify-content:center
1240 20 62 | items-center flex justify-between | align-items:center ; display:flex ; justify-content:space-between
1232 28 44 | items-center flex grow | align-items:center ; display:flex ; flex-grow:1

## 3. Collisions

No CSS selector in `ui/**` defines a class named as a v1 utility (checked every `.class` in selectors). The collision test in the spec can pass today.
HTML `class=` tokens were not checked (`ui/**/*.html`, 32 files); a token that matches a v1 name but has no CSS rule is harmless to the CSS rule set, but must be checked before the compiler hook ships.

## 4. Names to ADD and DROP

ADD (>=15 rules, not in spec, need no new token):
- `text-md` (13px, 46 rules, already matched by the spec's `text-N` family; the spec's text list omits `md`).
- `text-ellipsis` (`text-overflow:ellipsis` alone, 17 rules that are not in a `truncate` group).
- `bg-surface` (`background:var(--surface)`, 15 rules; `--surface` is defined in 3 places with different values).

ADD only after a token exists (token first, section 5): `rounded` 4px (25 rules), `rounded` 8px (18), `text-white` `#ffffff` (18),
`#ffffff14` as `background` (20) and `background-color` (15), `h-[31px]` (19; arbitrary value already allowed, no name needed).

DROP (<3 rules, or duplicate): `inline-flex` (0), `flex-1` (0; use `grow`), `shrink-0` (0), `select-none` (0), `hidden` (2), `opacity-N` (<=2 per value; keep only if a surface needs it),
`border-rim` (duplicate of `border`).
Keep but watch: `items-start` 4, `items-end` 5, `items-stretch` 5, `rounded-full` 5, `font-medium` 18.

## 5. Hardcoded literals to tokenise before utilities freeze them

px (declarations in width, height, padding, margin, gap, font-size, border-radius, top/left/right/bottom, min/max-width, inset; `:root` excluded; 1921 occurrences, 181 distinct):
`0px` 248, `12px` 117, `14px` 72, `8px` 69, `2px` 63, `10px` 59, `30px` 57, `16px` 55, `18px` 55, `13px` 53, `6px` 52, `4px` 51, `5px` 48, `9px` 43, `1px` 38, `11px` 36, `28px` 33, `24px` 32, `3px` 30, `7px` 27.
Off the 2px grid (odd px): 1, 3, 5, 7, 9, 11, 13 = 275 occurrences. These need a token or an arbitrary value; the v1 scale cannot express them.
Radius: literals 4, 8, 12, 5 do not match `--radius-sm..xl` (3, 6, 9, 11). Retire or remap one surface at a time (DESIGN_TOKENS.md section 1).

Hex (outside `:root`): 126 occurrences, 66 distinct. Top: `#ffffff` 16, `#202020` 7, `#87919b` 5, `#ffffff1f` 4, `#232629` 4, `#1c1c1c` 4, `#e4e9fc` 4, `#ffffff14` 3, `#1c2740` 3, `#131415` 3.
The spec's 328 hex count includes inline `<style>` and other sources; this table covers `ui/**/*.css` only.

## 6. Recommended pilot surfaces

| surface | sheet | rules matched | rule bytes | replaceable (gross) bytes | notes |
|---|---|---:|---:|---:|---|
| shell dock | `ui/shell/flamewm.css` (36826 B) via `ui/shell/dock.html` | 9 | 731 | 597 (82 %) | 8 classes; smallest, safest, lowest payoff |
| settings | `ui/settings.css` (16034 B) via `ui/settings.html` | 139 | 11118 | 6603 (41 % of sheet) | 105 classes; no inline `<style>`; best payoff |

Recommendation: pilot the dock (shell surface, proves the compiler hook on a shell page) and the settings sheet (proves bytes at scale).
The dock is 0.6 KB gross, so its real win is correctness, not bytes. Expect net savings lower than gross once class tokens are added to HTML.
Both sheets are touched by the design-tokens worker (`ui/shell`, `ui/settings`): rebase before each edit.

## 7. Skipped or not checked

- HTML `class=` collisions and inline `<style>` in `ui/**/*.html` (only the pilot pages were measured).
- Compiled `.rwr` size, compile ms, and layout rects: not measured here (Phase 2 scope).
- Net byte savings: gross only; no HTML rewrite.
- `flex-1`, `opacity-N` variants beyond the ones listed: not searched exhaustively.
