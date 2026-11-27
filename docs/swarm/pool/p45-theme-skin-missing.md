# Unit p45-theme-skin-missing
STATUS: DONE

Crates flamewm-skin and flamewm-ui: missing or corrupt theme/skin/icon theme: fall back to built-in default theme, report once. Audit loaders for unwrap/expect/early abort.

## Result
crates/flamewm-skin/src/lib.rs:19 | skin contract is a compile-time constant (DEFAULT = SkinMetrics::RWR_0_0_9); no file I/O, no deserialization, no loader | none — built-in default is the only source | none; no runtime path altered
crates/flamewm-skin/src/palette.rs:94,127 | Palette::default()/for_appearance() are total over Dark/Light; only unwraps are PoisonError::into_inner lock recovery | none | none; pixel values unchanged
crates/flamewm-skin/src/icons.rs:69 | IconRole::source() maps to static bundled asset paths; no external icon theme is read | none | none
crates/flamewm-ui/src/components/icon.rs:8 | Icon is a const role wrapper; flamewm-ui has no theme/skin loader, fs access, or panic site | none | none
crates/flamewm-shell/src/cursor_theme.rs:12,44 | out-of-scope owner already reports once via RepeatLog and rolls back to the previous theme on failure | none (informational only; no edit outside scope) | none
.vendor/kwin-master/src/pointer_input.cpp:1247 | precedent: requested cursor theme empty -> warn once -> default theme -> fallback theme, no abort | none (read-only reference, not a dependency) | none

PASS — audited every .rs file in flamewm-skin and flamewm-ui: both crates are compile-time token/role definitions with built-in defaults and contain no runtime theme, skin or icon-theme loader, no unwrap/expect/abort on external input, so no fallback fix is applicable there and zero production edits were made (nothing to build). Missing/invalid accent values already fall back to the product default in flamewm-api settings and flamewm-ui-core; cursor and icon-theme fallback plus report-once logging are owned by flamewm-session-core/flamewm-shell and are already implemented. Precedent recorded from .vendor/kwin-master (requested theme -> single warning -> default -> fallback). Skipped: no tests or builds were run (per unit rules); visuals untouched.
