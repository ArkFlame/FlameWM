# FlameWM Pass 10 — Desktop conversion compilation contract repair

## Exact user-reported failure

`./xephyr` aborts during `[1/5] Building ...` with three independent but related errors:

- `E0432` `flamewm_desktop_core::conversion` unresolved at `crates/flamewm-desktop/src/main.rs:19`.
- `E0599` `EntryMenuAction::Convert` not declared at `crates/flamewm-desktop/src/main.rs:2045`.
- `E0061` `entry_context_menu(is_trash, convertible)` calls a single-argument function at `crates/flamewm-desktop/src/main.rs:2158`.

**Root cause:** the live checkout has the newer desktop frontend (Pass 7+) but an older `flamewm-desktop-core` (pre-conversion). The complete Pass 9 archive already contains the correct `conversion.rs`, `pub mod conversion;`, `EntryMenuAction::Convert`, and two-argument menu function. The live checkout was therefore incompletely merged; the three files have to be brought forward together. This is not caused by Xephyr or the warning about `stroke_rounded_rect`.

## Smallest safe source repair (recommended for existing Git checkout)

From the repository root, place the standalone companion patch in a known path and run:

```sh
git apply --check /path/to/FlameWM_0.1.0_desktop_conversion_E0432_E0599_E0061_repair.patch
git apply /path/to/FlameWM_0.1.0_desktop_conversion_E0432_E0599_E0061_repair.patch
python3 -I tools/verify_desktop_conversion_contract.py
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
./xephyr
```

The companion patch contains **only**:

1. `crates/flamewm-desktop-core/src/lib.rs` — exports `conversion`.
2. `crates/flamewm-desktop-core/src/file_actions.rs` — adds `Convert` with label/menu policy and test updates.
3. `crates/flamewm-desktop-core/src/conversion.rs` — native Rust policy module for ImageMagick-backed conversion.

It was generated against `027_19_10_26_deepseek_branch.tar.gz`, applied with `git apply --check && git apply`, and compared **byte-for-byte** against the Pass 9 source for all three files. `git apply --check` is deliberately required: if the current checkout has modified these files since DeepSeek, STOP and merge those differences rather than overwriting others' work. Do not run `git reset --hard` or replace unrelated working-tree files.

If you prefer extraction rather than patching, use the **small repair overlay** archive against the Git checkout, but first examine local edits to those three specific files because unpacking replaces them. Never extract the full Pass 10 archive directly onto an actively edited checkout without reviewing the diff.

## Why this pass changes the source archive

- Adds `tools/verify_desktop_conversion_contract.py` source preflight, wired into `scripts/xephyr` when building, so the same partial-merge problem is identified before Cargo compiles.
- Adds `tools/test_desktop_conversion_contract.py` with four focused offline tests covering the complete contract and independent incomplete states.
- Includes the proven three-file repair patch at `tools/patches/desktop_conversion_E0432_E0599_E0061.patch`.
- Does **not** modify working conversion logic or recent workspace navigation, context-menu lifecycle, UI, or session behavior.

## Verification boundary

Run Rust/Cargo and Xephyr on your Linux machine; this source-export environment has no Cargo or rustc. The new preflight verifies source API consistency **only**, not successful Rust compilation, ImageMagick codecs, or native X11 runtime correctness. The earlier `stroke_rounded_rect` dead-code warning is non-fatal and outside the reported compilation failure.
