# Unit p44-font-missing-everywhere
STATUS: OPEN

Crates flamewm-render-core, flamewm-terminal, flamewm-render-x11: no system font found: use an embedded fallback glyph set and one report; never panic on empty font. Verify every find_font user.
