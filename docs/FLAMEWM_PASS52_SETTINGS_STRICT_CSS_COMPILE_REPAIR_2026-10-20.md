# FlameWM Pass 52 — Settings strict CSS compilation repair

Baseline: `FlameWM_0.1.0_PASS51_TITLEBAR_RUST_TEST_COMPILE_FIX_2026-10-20.tar.gz` (source overlay, version 0.1.0). This update supersedes Pass 51; do not apply older overlays afterward.

## Reproduced failure from supplied native log

`CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` failed while running `crates/flamewm-settings/build.rs`:

```
failed to compile .../ui/settings.html: unsupported CSS property 'text-align'
```

Although the error mentions `settings.html`, the declaration came from `ui/settings.css`:

```
#display-scale-value { min-width:52px; text-align:center; }
```

The strict FlameWM Render compiler `crates/flamewm-render-compiler/src/compile.rs` has no `text-align` CSS support, by design. The Settings scale percentage already uses `.value-pill`, which has `display:flex`, `justify-content:center`, and `align-items:center`. Thus `text-align:center` was redundant.

## Exact correction

- `ui/settings.css`: remove only `text-align:center` from `#display-scale-value`.
- `tools/test_flamewm_pass52_settings_css_contract.py`: add two regression checks ensuring the actual Settings scale label uses the supported flex centering and that every declared property in `ui/settings.css` is recognized by the compiler.
- This document records evidence; no runtime code or layout ownership changed.

## Verification here

- Original baseline reintroduced `text-align:center` in an isolated local negative check: new tests fail (2 failures) as expected.
- Restored corrected source: Pass 52 new regression tests, 2/2 pass.
- Existing Settings acceptance schema tests, 155/155 pass.
- Pass 51 chrome compile contract, 2/2 pass.
- Pass 50 integration source contract, 9/9 pass.
- Source archive tests, 6/6 pass.
- Package pipeline tests, 7/7 pass.
- Total selected Python checks: **181/181 passed**.
- Static audit of all Settings CSS declarations found 39 distinct properties; the only previously unrecognized property was `text-align`; after the edit, none remain unknown by property name.

**NOT RUN:** Rust compiler/build scripts, full Cargo suite, graphical Settings validation, Debian production packaging. These require your native complete asset-bearing checkout. `ui/assets` symlinks and historical assets are retained from Pass 51; the release is an **integrated source overlay**, not a standalone build distribution.

## Safe installation on the complete local checkout

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS52_SETTINGS_STRICT_CSS_COMPILE_FIX_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
  tar --touch -xzf "$ARCHIVE" -C . && \
  CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
  CARGO_BUILD_JOBS=2 cargo test -p flamewm-render-compiler -p flamewm-settings && \
  CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Do not reapply Pass 41 or any prior source archive afterward: doing so can revert new fixes. If compilation reports a new error, preserve the exact first error and repair that distinct problem, not the now-solved CSS property.
