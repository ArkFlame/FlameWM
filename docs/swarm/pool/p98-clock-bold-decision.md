# Unit p98-clock-bold-decision
STATUS: CLOSED

p93: the Clock widget default `bold=true` (crates/flamewm-widgets-core/src/widgets/kind.rs:124) forces the Bold face at startup in flamewm-desktop. Find out whether the user-visible clock is bold by default and whether a Regular fallback or lazy-load can keep the exact pixels (same face, loaded when first painted, which is startup anyway). Conclude whether any RAM can be saved with ZERO visual change; if not, say so and close. No edits to defaults.

## Result

Decision: NO-GO. No zero-visual-change RAM saving from a Regular fallback or lazy Bold load. Closed.

Evidence (read-only, no builds or tests run):
- Default is bold: `crates/flamewm-widgets-core/src/widgets/kind.rs:124` `OptionSpec::toggle("bold", "Bold time", true)`.
- Clock paint: `crates/flamewm-desktop/src/widgets/sync.rs:79` sets `font_weight` to 700 when `bold` is set, else 400, applied to the time nodes each tick.
- Clock is on the default desktop, so the Bold weight is requested on the first painted frame at startup. Lazy-loading the Bold face therefore moves nothing out of the startup set, and the saving is zero.
- Bold is also used by UI defaults: `crates/flamewm-desktop/src/projection.rs:91-112` reads `fontBold` for global font defaults, and `sync.rs:562` uses the bold flag for text cards.
- A Regular fallback changes the glyph weight, so it is a visible change and is ruled out by the unit.

Not checked: the exact renderer font-cache internals (`crates/flamewm-wm-wayland/src/font.rs` only loads a title font). Any saving would need a measured RSS diff, which this read-only unit did not run. No edits made to defaults.
