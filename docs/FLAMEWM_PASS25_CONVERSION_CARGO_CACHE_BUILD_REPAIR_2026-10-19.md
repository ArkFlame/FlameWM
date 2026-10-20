# FlameWM Pass 25 — E0432 / E0599 / E0061 build repair

## Exact failure and first canonical owner

In a checkout originally compiled before Desktop Convert, bare `cargo check --workspace --all-targets`
can use an obsolete, already-compiled `flamewm-desktop-core` library. The desktop binary expects
`flamewm_desktop_core::conversion`, `EntryMenuAction::Convert`, and
`entry_context_menu(is_trash, convertible)`. Those symbols **are present in Pass 24's current
source**. The error pattern is thus consistent with cached object/API mismatch after overwriting
source files with archived (1970) mtimes; an incompletely overlaid local tree is the other case
and must be rejected by the source gate.

The downstream E0282 type-inference errors come from losing these types/functions in the
compiler's linked core API. Do not change the desktop caller's working Convert behavior to fit
obsolete symbols.

## Permanent recovery in this pass

- `scripts/build-deb` now validates the source/module/Cargo path and forcibly invalidates only
  `flamewm-desktop-core` and `flamewm-desktop` **release** artifacts before compiling.
  The `--force` is intentional: an existing content-hash marker does not guarantee that
  the Cargo artifact was rebuilt correctly in a checkout with archived source mtimes.
- New `scripts/check-cargo` does the equivalent scoped invalidation for the **dev** profile
  before `cargo check --workspace --all-targets`.
- `verify_desktop_conversion_contract.py` additionally requires `convert_file` and
  `predicted_destination`, both consumed by the newer desktop projection.
- The Pass 25 handoff `.tar.gz` ships updated Cargo-source file timestamps instead of 1970,
  so an extracted overlay is more likely to trigger Cargo's normal mtime-based recompilation.
  Reproducible source-release artifacts created by `scripts/package-source-archive` still use
  `SOURCE_DATE_EPOCH`; the safe-build entrypoints remain necessary for those archives.

## Full-checkout commands

Overlay the Pass 25 tar source on your **full checkout**, preserving its existing `assets/`.
Then run:

```sh
./scripts/check-cargo
./scripts/build-deb
sudo apt install --reinstall ./.target/dist/flamewm_0.1.0_amd64.deb
```

`./scripts/build-deb` independently repairs the *release* cache: `./scripts/check-cargo`
is only needed if you want a development `cargo check`. Do not use a failed `&&` chain and
then accidentally install an older previously built `.deb`.

## Verification boundaries

Source-contract and offline packaging-harness tests run without a Rust toolchain. The actual
Rust workspace compile, Xephyr and SDDM runtime cannot be certified in the authoring environment.
