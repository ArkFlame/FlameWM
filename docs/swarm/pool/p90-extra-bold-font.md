# Unit p90-extra-bold-font
STATUS: OPEN

The new flamewm-shell parses an extra font IBMPlexSans-Bold.ttf at startup that the older build did not (fonts opened: Regular, SemiBold, NotoSans-Bold, now also IBMPlexSans-Bold). A fontdue parse costs MBs of heap. Read crates/flamewm-render-core/src/glyphs.rs font selection (CANDIDATES, load, weight handling) and find which text requests weight 700 at shell startup (ui/shell/*.html font-weight, projection code). Decide: reuse an already-loaded face, or load Bold lazily on the first bold glyph. Visual output identical. At most 2 edits, UNBUILT.
