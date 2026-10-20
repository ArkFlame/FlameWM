# FlameWM 0.1.0 Pass 36 — Control-plane panel bootstrap test alignment

## Failing native gate

`cargo test -p flamewm-platform -p flamewm-control-core -p flamewm-wm-x11` failed in `flamewm-control-core/tests/platform_control.rs::shell_bootstrap_reconciles_display_and_panels_before_reply`:

```text
actual:   Rect { x: -1346, y: 593, width: 1342, height: 44 }
expected: Rect { x: -1350, y: 597, width: 1350, height: 44 }
```

## Root cause and correction

This is an obsolete assertion, **not** faulty panel placement. `ProductSettings::default()` selects `TaskbarStyle::Floating`, `taskbar_floating_margin: 4` and `taskbar_height: 44`. `PlatformHost::start()` and `resync_panels_for_displays()` correctly load that frame; the panel's canonical `OutputPanel::panel_rect()` places it within the current output, not across its entire width.

For `Rect::new(-1350, 0, 1350, 641)` at 100% scaling, the correct panel is `Rect::new(-1346, 593, 1342, 44)`:

```text
panel.x = output.x + 4 = -1346
panel.y = output.bottom - 4 - 44 = 593
panel.width = output.width - 2 * 4 = 1342
panel.height = 44
```

The test is updated to assert that geometry and the original output identity. **No production behavior was changed.** Changing production geometry to satisfy the obsolete expectation would break the already-accepted floating taskbar design, edge reservation, and other tests.

## Scope, regression and safety

The integration-test assertion, one stale Pass 33 static guard, and this explanation were changed. The Pass 33 guard previously required mode IDs to be nonzero, contradicting the authorized Pass 35 Xephyr fallback; it now asserts valid/connected output selection and both Xephyr and disabled-output regression coverage. Native controls, panel/layout/rendering, monitor configuration, session, Brave/KWallet, and packaging are untouched. The test continues to verify that a moved negative-origin output reaches the initial `GetShellBootstrap` response with a panel on that output, rather than returning stale geometry.

The user's native run already proved `cargo check --workspace --all-targets` succeeds; this archive requires a fresh Cargo test and Debian package check on that machine. The creation environment does not have the Rust toolchain, Xephyr, or a physical display and cannot certify their outcomes.

## Archive format and packaging caveat

Pass 36 is a minimal **overlay delta** for an existing complete Pass 35 checkout, not a standalone source archive. The Pass 35 source overlay supplied to this task omits `.vendor/RustWebRender-0.0.9` executable and `assets/web` files referenced by `packaging/source-manifest.txt`. Thus, running that manifest over an *isolated extraction of the overlay* fails for missing assets; those files are expected from the user's complete repository and are deliberately neither fabricated nor removed. Run `./scripts/build-deb` against the actual complete checkout.

Recommended verification:

```bash
CARGO_BUILD_JOBS=2 cargo test -p flamewm-control-core --test platform_control shell_bootstrap_reconciles_display_and_panels_before_reply && \
CARGO_BUILD_JOBS=2 cargo test -p flamewm-platform -p flamewm-control-core -p flamewm-wm-x11 && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb && \
./scripts/xephyr
```
