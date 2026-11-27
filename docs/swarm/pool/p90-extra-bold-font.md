# Unit p90-extra-bold-font
STATUS: PARTIAL

The new flamewm-shell parses an extra font IBMPlexSans-Bold.ttf at startup that the older build did not (fonts opened: Regular, SemiBold, NotoSans-Bold, now also IBMPlexSans-Bold). A fontdue parse costs MBs of heap. Read crates/flamewm-render-core/src/glyphs.rs font selection (CANDIDATES, load, weight handling) and find which text requests weight 700 at shell startup (ui/shell/*.html font-weight, projection code). Decide: reuse an already-loaded face, or load Bold lazily on the first bold glyph. Visual output identical. At most 2 edits, UNBUILT.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-core/src/glyphs.rs:14-20, 145-222 | fontdue path loads exactly one face (first CANDIDATES hit: IBMPlexSans-Regular, then Noto, DejaVu, Liberation, Cantarell; bundled DejaVuSansMono fallback). It has no weight parameter at all, so it never opens a Bold file. | none | none |
| crates/flamewm-wm-wayland/src/font.rs:12-18 | Same one-face fontdue loader with the same CANDIDATES. No weight handling, no Bold file. | none | none |
| crates/flamewm-render-x11/src/xft/backend_measure.rs:57-88 | The only weight-to-face mapping. `font_for` keys on (pixels, normalize_weight(weight)) and asks fontconfig via XftFontOpenName for `weight=bold` (700+), `semibold`, `medium`, `regular`. Fonts are opened lazily on first use and cached per key. fontconfig, not our code, picks IBMPlexSans-Bold or NotoSans-Bold. | none | none |
| crates/flamewm-ui-document/src/template.rs:270, 448 | `font_weight(id, 700)` is a DOM-style contract call. Start view calls it at crates/flamewm-shell/src/async_projection/start_view.rs:130 (700) and :144/:152 (400). It is a request, not a file open. | none | none |
| ui/shell/*.html, ui/desktop/desktop.css | Many `font-weight:700` rules (e.g. desktop.css:51, 59, 69-77). These are CSS and are only compiled by the render-compiler into node weights. No font file is named. | none | none |

Finding: in this checkout no code path loads `IBMPlexSans-Bold.ttf` through fontdue, so a lazy-Bold change to glyphs.rs would have nothing to defer. The only Bold open is the Xft path, which already loads on first bold request and is cached per (size, weight). Reusing a loaded face is not possible for Bold in fontdue because only Regular is parsed. The extra MBs described in the unit are therefore not from glyphs.rs or font.rs in this tree; they likely come from a shell build that differs from this checkout, or from fontconfig/Xft under the X11 path, which the brief did not cover.

No edits made (0 of 2 allowed). Nothing built or tested. Next unit to check: confirm which binary opened IBMPlexSans-Bold (strace -e openat on the shell at startup, outside gates) before changing any font code.

PARTIAL: investigation complete, no production change, because the cited fontdue Bold load does not exist in this source. All edits: none, UNBUILT.
