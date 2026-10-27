# FlameWM Pass 53 — desktop scale-call E0061 after obsolete Pass 41 replay

**Failure supplied:** `flamewm-desktop/src/main.rs` called `projection::project_item` with seven arguments while `projection.rs` requires eight: `scale: u16`. The supplied command *repeatedly extracted Pass 41*, a historic two-file hotfix that predates per-output desktop scaling. Its `main.rs` restores the seven-argument caller and old icon-slot bookkeeping.

**Correct source:** Pass 52 already has the required call in `sync_document`:

```rust
let output_grid = grid_for_output(state, output);
// ...
let key = projection::project_item(
    document, &mut state.icons, index, kind, &label, point,
    icon.as_deref(), output_grid.scale_percent,
)?;
state.icon_slots[index] = Some(key);
```

Pass 53 **repackages that already-correct, latest source**, with `tools/test_flamewm_pass53_desktop_scale_call_contract.py` added to explicitly reject this rollback. No other Rust changes are warranted by this error. The scaling value is monitor-scoped; do not hardcode `100` and do not revert to the historical icon-key path.

## Verification

- Inspected Pass 41: seven-argument caller and `icon_slots` recomputation, reproducing the mismatch reported.
- Inspected Pass 52: caller supplies `output_grid.scale_percent` and stores returned `IconKey`; the callee expects `scale: u16`.
- Pass 53 regression guards enforce matching caller and callee signatures.
- Archive is a **source overlay**, not a standalone application: it lacks the complete runtime `assets/` from the user's real checkout. Rust compiler, graphical Xephyr, live SDDM and Debian packaging are unavailable in the authoring container, so **native build and runtime acceptance are NOT_RUN**.

## Correct application and native verification

**Do not extract Pass 41 or any other older overlay afterward**. Save Pass 53 to `~/Downloads` and run:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS53_DESKTOP_SCALE_E0061_REBASE_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
  tar --touch -xzf "$ARCHIVE" -C . && \
  python3 tools/test_flamewm_pass53_desktop_scale_call_contract.py && \
  CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
  CARGO_BUILD_JOBS=2 cargo test -p flamewm-desktop && \
  CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

The old Pass 41 extraction, not a missing feature in the latest code, caused this specific compiler error. Cargo may reveal a different error later; record the first genuine error instead of reapplying old archives.
