# FlameWM 0.1.0 — Pass 14: strict RWR desktop CSS build repair

## Captured failure

The user's full checkout now passes desktop-core contract checks and targeted Cargo cache invalidation, but the `flamewm-desktop` build script exits 101:

```
error: failed to run custom build command for `flamewm-desktop v0.1.0`
thread 'main' panicked at crates/flamewm-desktop/build.rs:76:6:
compile desktop UI: "unsupported CSS property 'text-align'"
```

This is a **source/CSS compiler contract failure**, not the previous `E0432/E0599/E0061` API mismatch, missing RWR node, or evidence of stale Cargo artifacts. Do not repeatedly clear the entire Cargo cache. `crates/flamewm-render-compiler/src/compile.rs` accepts `display:flex`, `justify-content:center`, and `align-items:center`, but has no `text-align` declaration. The sole `text-align` occurrence in active desktop UI styles is the Convert wheel option rule in `ui/desktop/desktop.css`.

## Minimal corrective implementation

- `ui/desktop/desktop.css`: replace `text-align:center` with supported `display:flex; align-items:center; justify-content:center` on `.context-menu .convert-wheel-option`. Preserve the existing dimensions, positioning, colors, border and hit-target semantics.
- `crates/flamewm-render-compiler/src/compile.rs`: permanent unit test loads **real** `ui/desktop/desktop.css` and `ui/shared/menu.css`, compiles an actual representative Convert wheel button in **strict mode**, and asserts the compiled typed style is `Display::Flex`, `JustifyContent::Center`, `AlignItems::Center`.
- `tools/test_desktop_convert_css_contract.py`: tiny offline regression that checks the actual production CSS wheel rule and canonical compiler support. Its initial run on unmodified Pass 13 failed on `text-align` (1 failure / 2 tests); its post-fix run passed (2/2).
- `scripts/xephyr` and `scripts/check`: execute that inexpensive offline regression before Cargo cache cleanup/build. This avoids another expensive cleanup cycle on the same unsupported CSS defect; the authoritative strict Rust compiler remains the ultimate build gate.

## Results in this isolated export

- **688 offline Python tests passed**, excluding only `test_architecture_guard` and `test_flamewm_settings_fake_portal` suites (see blockers below).
- `python3 -I tools/test_desktop_convert_css_contract.py -q`: 2 passed.
- `python3 -I tools/verify_desktop_conversion_contract.py`: passed; Pass 13 desktop-core source hashes unchanged.
- `python3 -I tools/ui_contract_guard.py`: 0 errors, 24 warnings.
- `bash -n scripts/xephyr scripts/check`: passed.
- Original `text-align` regression: red **before** editing and green after changing the CSS declaration.
- The original Pass 13 archive is preserved; additional files are the CSS regression test and this report.

### Pre-existing/environmental gates not claimed as fixed

- This isolated runtime has **no Cargo/rustc** and the supplied source archive omits the complete asset directories; it cannot run `cargo check`, compile the desktop RWR or launch Xephyr. Successful native compilation must be reported by the agent/user in the full checkout.
- `test_architecture_guard.ArchitectureGuardNegativeTest.test_clean_copy_passes` is red on unmodified Pass 13 due to `ui/desktop/desktop.css [j12-g03-no-display-none-transient]`, traced to the existing `.sticky-title { display:none; }` rule. Not introduced by Pass 14 and not modified without a separate source/visual authority review.
- `test_flamewm_settings_fake_portal` needs the unavailable Python `dbus` module and working `XDG_RUNTIME_DIR` in this isolated container. The broad unfiltered suite failed those 4 environment-dependent test cases. These failures are not evidence about the CSS fix.
- `flamewm-render-x11/src/xrender.rs:202` unused `stroke_rounded_rect` is an existing nonfatal warning.

## Apply safely in the live repository

Do not overwrite the checkout with this trimmed export; preserve other agents' uncommitted changes. Use the Pass 13 → Pass 14 patch after checking against current source:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender
git status --short
git apply --check ~/Downloads/FlameWM_0.1.0_pass13_to_pass14_strict_css_convert_fix.patch
git apply ~/Downloads/FlameWM_0.1.0_pass13_to_pass14_strict_css_convert_fix.patch
CARGO_BUILD_JOBS=2 cargo check -p flamewm-desktop --all-targets
RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-compiler desktop_convert_wheel_css_compiles_with_native_flex_label_centering
./xephyr
```

If the patch does not apply, do not force it; inspect overlapping CSS/compiler changes and manually merge the five small source changes. Native `./xephyr` success and a visual check of centered wheel labels are required to close this incident.
