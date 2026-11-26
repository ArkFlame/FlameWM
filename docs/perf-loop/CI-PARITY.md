# CI parity for the perf loop

What `.github/workflows/*.yml` actually runs, what `scripts/perf-loop check` does not run,
and the cheapest local command that closes each gap. Read-only audit of the workflows; no
workflow change is proposed.

## 1. Every command CI runs

### `ci.yml` — every push to `main` and every pull request

| # | Command | Gate |
|---|---|---|
| 1 | `python3 -I tools/architecture_guard.py` | architecture boundaries (one owner per mechanism, `.vendor` never a dependency) |
| 2 | `python3 -I -m unittest discover -s tools -p 'test_architecture_guard.py' -q` | the guard's own tests |
| 3 | `apt-get install pkg-config libdbus-1-dev libpulse-dev libx11-dev libxi-dev libxft-dev libxkbcommon-dev libinput-dev libgbm-dev libdrm-dev libudev-dev libwayland-dev` | build environment, not a gate |
| 4 | `rustup` nightly + `rustfmt`, `clippy` components | toolchain selection, not a gate |
| 5 | `cargo fmt --all -- --check` | formatting |
| 6 | `cargo check --workspace --all-targets` | type/borrow check of lib, bins, tests, benches |
| 7 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | lints, **all features**, warnings are errors |

`ci.yml` runs **no** `cargo test`. Test execution lives only in `scripts/check`/`scripts/gate`,
not in CI.

### `arch.yml` — Arch package, install, runtime and visuals

1. `pacman -Syu --needed git namcap xorg-server-xvfb xorg-xprop xorg-xwininfo`
2. `./scripts/build-arch --syncdeps --noconfirm` — makepkg build of `flamewm-git`, then `namcap`
3. `pacman -U` the package, `pacman -Qkk flamewm-git` (file integrity)
4. for every installed ELF in `/usr/bin` and `/usr/lib/flamewm`: `ldd "$binary" | grep 'not found'` must be empty
5. `flamewm-wayland --capabilities | grep -Fx 'drm_compiled=true'`
6. `grep -Fx 'Exec=...' /usr/share/xsessions/flamewm.desktop` and `/usr/share/wayland-sessions/flamewm-wayland.desktop`
7. X11 session smoke on `Xvfb :91`: `xprop -root _NET_SUPPORTING_WM_CHECK`, `pgrep -x flamewm`,
   `pgrep -x flamewm-shell`, `pgrep -x flamewm-desktop`, `xwininfo -root -children`
8. `python3 -I tools/flamewm_visual_tests.py --layout both --display 93 --out visual` — one and two screens
9. `pacman -R flamewm-git`, then assert `/usr/bin/flamewm-session`, the xsession desktop file and
   `/usr/lib/flamewm` are gone

### `dist.yml` — distribution build (`workflow_dispatch`, and `workflow_call` from release)

- `./scripts/build-arch --syncdeps --noconfirm` + `find .target/dist -name 'flamewm-git-[0-9]*.pkg.tar.zst'`
- `./scripts/build-deb` (re-executes inside the pinned container from `packaging/target-baseline.toml`)
- `./scripts/build-cpak --engine docker --image ghcr.io/<owner>/flamewm-terminal [--push]`
- `cd dist && sha256sum -- * >SHA256SUMS`

### `release.yml` — tags `v*`

- `./scripts/version`, then fail unless `GITHUB_REF_NAME == "v${version}"`
- `dist.yml` via `workflow_call` (`push_cpak: true`)
- `cd dist && sha256sum --check SHA256SUMS`, then `gh release create --generate-notes`
- AUR job (needs `AUR_SSH_PRIVATE_KEY`): copy `packaging/arch/PKGBUILD`, `makepkg --printsrcinfo > .SRCINFO`,
  commit and push to `flamewm-git`

## 2. What `scripts/perf-loop check` does not run

`scripts/perf-loop check` is exactly one command:

```
cargo check --workspace --all-targets --message-format=short
```
with only `error` lines printed, and cargo's exit code returned.

Therefore it does **not** run:

| Gap | Why it matters | Covered by |
|---|---|---|
| `cargo fmt --all -- --check` | a formatting diff lands in the pushed commit; CI fails after the push | CI step 5 |
| `cargo clippy ... -- -D warnings` | lint-only failures (unused `mut`, `needless_range_loop`, `collapsible_if`, …) pass locally and fail CI | CI step 7 |
| `--all-features` compilation | feature-gated code is never built: `flamewm-wm-wayland/drm` (DRM/KMS/GBM/libinput backend), `flamewm-integrations-linux/pulse-audio`, `flamewm-ui-x11/test-support`; note `flamewm-lock/x11` is a default feature, so the `no-x11` core build is compiled by neither side | CI step 7 |
| `python3 -I tools/architecture_guard.py` + its unittest | a new cross-boundary import or a new `.vendor` dependency passes `cargo check` and fails CI first | CI steps 1–2 |
| `cargo test` | not a CI gap (CI has none) but the shared `scripts/check`/`scripts/gate` treat it as required | — |
| makepkg build, `namcap`, `pacman -U`/`-Qkk`, `ldd` closure, capability/desktop `Exec` greps | packaging-only; needs an Arch container and root | `arch.yml` |
| Xvfb X11 session smoke, `tools/flamewm_visual_tests.py` one/two screens | runtime/visuals only | `arch.yml` |
| `scripts/build-deb`, `scripts/build-cpak`, `sha256sum` collection | release artifacts only | `dist.yml` |
| tag↔version equality, `sha256sum --check`, AUR `--printsrcinfo` | release-only | `release.yml` |

Note the asymmetry that bites the loop: CI's cheapest Rust gate compiles `--all-features`, while the
local loop compiles default features only. A `#[cfg(feature = "drm")]` or `pulse-audio` edit is
green locally and red in CI.

## 3. Cheapest local equivalent per gap

All of these already exist; nothing new is required. Ordered by cost.

| Need | Cheapest local command | Cost |
|---|---|---|
| fmt + clippy `-D warnings` + architecture guard for the crates a slice touched | `scripts/gate <crate> [crate ...]` | one shared cargo-lock slot; runs guard, fmt, clippy `--all-targets -D warnings`, then `cargo test` for those packages only if the earlier steps pass |
| full `ci.yml` parity (guard + unittest, fmt, check, clippy `--all-features -D warnings`), CI order, fails on first error | `scripts/lint` | whole-workspace check + all-features clippy |
| all-features compile of just the changed crate and its reverse dependencies | `CARGO_TARGET_DIR=<shared> cargo clippy -p <crate> --all-targets --all-features -- -D warnings` | smallest all-features probe |
| fast iteration between edits | `scripts/dev-check.sh [BASE_REF]` | changed crates + reverse deps, check only; **not** parity |
| compile gate the perf loop already has | `scripts/perf-loop check` | one workspace check (default features) |
| tests (not in CI, but in `scripts/check`/`scripts/gate`) | `scripts/gate <crate>` | included there |
| makepkg/SRCINFO without building | `./scripts/build-arch --printsrcinfo` (needs `makepkg`; it forwards flags to makepkg) | seconds; no package build |
| AUR `SRCINFO` drift | `makepkg --printsrcinfo` in `packaging/arch/` and diff against the published `.SRCINFO` | seconds |
| installed-binary linkage closure | `ldd` closure, then `scripts/doctor` (pkg-config modules + `find_library` for Xft/fontconfig/Xrender/Xi) | seconds, uninstalled equivalents |
| runtime/visuals | `scripts/verify-visual` (fresh `scripts/xephyr` root screenshot), `scripts/look-capture <bin-dir> <out-dir>` (per-surface, paint-settled), `scripts/flamewm-nested-smoke` (Wayland) | minutes, nested only, never DRM/logind |
| distribution checksums | `scripts/dist-checksums [dist-dir]` | seconds |
| release gate | `scripts/release-gate`, `scripts/build-dist` | minutes; release only |

### Recommendation for the loop

1. Per slice: `scripts/gate <crate>` (background, wait on `GATE_DONE all=` in its log).
2. At the 30-minute build gate: `scripts/lint` **once** instead of `scripts/perf-loop check` — it is
   the same cargo work plus fmt, the guard, and `--all-features` clippy, i.e. exact `ci.yml` parity
   for a single extra lint pass on artifacts already warm.
3. Keep `scripts/perf-loop check` only for the sub-minute "did my edit compile" question between
   gates, and treat its green as compile-only, never as CI parity.
4. Never run makepkg/`build-deb`/`build-cpak`/`--printsrcinfo` inside the perf loop: they are release
   artifacts and the first two compile the whole tree again.
