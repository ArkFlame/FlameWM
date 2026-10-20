# FlameWM 0.1.0 — Pass 40 Codex continuation and release status

**Source of truth:** uploaded `1439_20_10_26.tar.gz`, SHA-256 `427f16ac695585e2596afec45c3963ddafe00b5c6b7dd9abe4172f602328e6ac`. This is Codex's in-progress 2026-10-20 source snapshot, not the complete asset-bearing local checkout. The Codex transcript ends after application/shell/settings/desktop/WM/session test successes while integration and release checks remain unfinished.

## Corrections in this continuation

1. Removed `crates/flamewm-wm/src/bin/desktop_compat.rs`, an obsolete source duplicated from pre-Pass-30 overlays. Session initialization continues to use `src/session_app_compat.rs` through the explicit `flamewm-session` target (`autobins = false`). Its old presence caused three regression checks to fail. The older helper must also be removed when applying this overlay to a checkout: `./scripts/apply-pass30-cargo-layout` is the existing, validated and idempotent migration.
2. Updated `tools/test_flamewm_pass24_regressions.py` to assert the actual canonical file-open launcher: `spawn_with_startup(...)` followed by `spawn_reaped_with_id(...)`, instead of the obsolete direct `spawn_reaped(&mut command)` assertion. The new expectation preserves MIME/KIO opening, notification, and child reaping.
3. Updated `tools/test_flamewm_deb_acceptance.py` to check the current SDDM `Exec=/usr/libexec/flamewm/flamewm-session-wrapper`, `TryExec=/usr/bin/flamewm-session`, and actual wrapper staging. The old test expected pre-Pass-32 direct session execution. The runtime wrapper/service lifecycle was **not changed**.

No production Rust code was modified in Pass 40. Codex's in-progress monitor-specific UI, wallpaper, MIME, launch feedback, lock-screen and logout changes are retained from the uploaded snapshot.

## Test evidence in this environment

- Python Pass series (`python3 -m unittest discover -s tools -p 'test_flamewm_pass*.py' -q`): **62 passed** after corrections (originally four failures).
- Selected acceptance (`python3 -m unittest -q tools.test_flamewm_lock_acceptance tools.test_flamewm_settings_acceptance_schema tools.test_package_source_archive tools.test_flamewm_deb_acceptance tools.test_flamewm_package_pipeline tools.test_ui_contract_guard`): **243 passed** after correction (originally one SDDM-wrapper expectation failure).
- `python3 tools/architecture_guard.py`: **FAIL (6 violations)**, existing in the supplied snapshot: `flamewm-applications/Cargo.toml [x11rb-dependency]`; `flamewm-applications/src/launch_feedback.rs [x11rb-symbol]`; `flamewm-applications/src/lib.rs [j11-g08-prefixed-env]`; `flamewm-desktop/src/projection.rs [j7-g5-no-magenta-colorkey]`; `flamewm-wm/src/session_app_compat.rs [j11-g08-prefixed-env]`; `ui/desktop/desktop.css [j12-g03-no-display-none-transient]`. These require a separate canonical-owner repair; relaxing the guard would not be proof.
- Full `python3 -m unittest discover ... test_*.py` was **not completed** (exceeded runtime timeout; several test modules are interactive/slow). Do not report it as passed.
- Rust Cargo, native tests, Debian packaging, Xephyr/physical monitor, and SDDM logout: **NOT RUN**, because Rust toolchain and graphical facilities are unavailable in this container.

## Mandatory release block: incomplete uploaded source

The provided `1439_20_10_26.tar.gz` contains **940 entries** and **no `assets/` or `.vendor/` entries**. Its UI symlinks point into absent `assets/`. The project packager requires, among other things, `assets/cursors/cursors` and `assets/web/breeze` (see `scripts/package` and `scripts/build-deb`). The earlier genuine Pass 37 complete checkout yielded **31,211 members** and about **18 MB** of compressed source, according to the Codex transcript, but that entire artifact was **not uploaded here**. It cannot be faithfully reconstructed from this partial source snapshot or generic upstream icon bundles. Do not rename this overlay into a "complete, build-ready source" archive or silently substitute assets.

## Apply to the user's original complete checkout, then build the genuine release

**Important:** this source overlay replaces matching source files and intentionally leaves the checkout's existing `assets/` and `.vendor/` untouched. Preserve a backup/commit of current dirty changes before applying. Place the newly named overlay in `~/Downloads`.

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS40_CODEX_CONTINUATION_SOURCE_OVERLAY_2026-10-20.tar.gz"

test -s "$ARCHIVE" &&
tar -tzf "$ARCHIVE" >/dev/null &&
test -d assets/cursors/cursors && test -d assets/web/breeze &&
tar --touch -xzf "$ARCHIVE" -C . &&
./scripts/apply-pass30-cargo-layout &&
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets &&
CARGO_BUILD_JOBS=2 cargo test -p flamewm-platform -p flamewm-control-core -p flamewm-wm-x11 -p flamewm-shell -p flamewm-settings -p flamewm-lock &&
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

On genuine success, the packager creates `.target/dist/flamewm-0.1.0-source.tar.gz` (from the full asset-bearing checkout), `.target/dist/flamewm-0.1.0-linux-x86_64.tar.gz`, `.target/dist/flamewm_0.1.0_amd64.deb` and `.target/dist/SHA256SUMS`. Look for `FLAMEWM-DEB-READY` before installing the .deb. The `tar --touch` option avoids the earlier extraction-timestamp/Cargo-cache trap.

## Product acceptance still required

Actual KDE/Dolphin Open With enumeration and Brave profile/wallet sharing; two-output popup/taskbar/window identity, Magic Lamp endpoints and lock security during hotplug; live typography updates; wallpaper sharing, Start slide clipping, and physical SDDM return-to-greeter. Codex's recorded unit test successes are valuable but **not a substitute for these runtime gates**. Never restart SDDM or terminate other sessions just to simulate logout acceptance.

**Status: source overlay ready; genuinely complete standalone source release BLOCKED by missing runtime assets in the supplied archive and native verification facilities.**
