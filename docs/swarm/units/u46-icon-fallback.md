# u46 icon fallback (orchestrator, UNBUILT)
| file:line | failure | now | change | risk |
|---|---|---|---|---|
| integrations-linux/src/icons/mod.rs generic_fallback | packaged `assets/web/flamewm-icon.svg` missing (broken install) made prepare_name("") fail, so blank UI | returns generated grey rounded square plus one-time stderr record | `builtin_glyph` + Err branch | low; app-miss transparent slot path (line ~861) untouched |
Test: builtin_glyph_is_opaque_inside_and_clear_at_corners (unrun). PARTIAL until built.
