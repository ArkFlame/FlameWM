# FlameWM 0.1.0 — Pass 34: source-level allocation reduction and release fix

**Input:** Pass 33 four-agent + desktop-services merged overlay.  
**Output:** Pass 34 replacement **overlay**, contains the complete Pass 33 overlay plus these bounded edits. Extract over the user's **complete** FlameWM repository (`assets/` and `.vendor/` are only partially represented in the overlay). Version remains `0.1.0`.

## Scope and concrete cause

The reported log is green for `cargo check --workspace --all-targets` and the optimized `cargo build --release --workspace`. Failure occurs **after** compilation, in `scripts/package-source-archive`: source manifest lists `scripts/` as part of the release, validates every executable under it, and rejects executable `scripts/apply-pass30-cargo-layout` because it was not registered. The fail-closed protection is correct. Pass 34 adds `@exec scripts/apply-pass30-cargo-layout` to `packaging/source-manifest.txt`. The packager remains strict; no chmod workaround, disabling validation, or deletion of the migration script.

A regression test performs the exact red/green repro with the real archiver and an executable copy: absent entry -> `ValueError: executable is not registered: scripts/apply-pass30-cargo-layout`, no archive published; registered -> valid `.tar.gz` containing the executable bit. A second contract scans all currently present `scripts/` and `tools/` executables against the real manifest. In the Pass 33 extracted overlay this was the **only** newly unregistered executable. Other release sources and installed assets must still be present in the user's full repository.

## Memory optimization (source-level, not RSS/PSS proof)

### Launch-feedback overlay: avoid repeated owned strings while a launch is pending

`crates/flamewm-shell/src/launch_feedback.rs` previously called `self.sequences.visible().cloned()` inside its active tick, generating owned copies of the startup ID, icon and WM_CLASS even when unchanged. It also constructed a fresh key `(sequence.id, icon.to_owned(), side)` and cloned that key again **on every active tick**, with a nominal 50 ms interval. 

Pass 34 reads the current sequence by reference, stores only the copied `Instant`, and uses `updated_launch_key` to allocate a new owned ID/icon pair only on a *real change of startup ID, icon name or badge size*. A completed raster still clones the requested key once into the painted state; the overlay lifecycle and 50 ms pointer interval are preserved. In a steady active tick with an unchanged launch, the request-key path now creates **zero new owned strings** instead of cloning multiple Strings. The improvement is observable by source inspection and a dedicated Rust unit test for equal/changing keys; no numerical reduction in MiB is claimed.

### Freedesktop startup notification: direct escaped-field construction

`crates/flamewm-applications/src/launch_feedback.rs::quote_field` previously used `flat_map(...vec![...])` for every input character and then formatted another output string. Now it writes quotes and escape characters directly to one preallocated String. This eliminates the temporary `Vec<char>` per processed character, preserves filtering of control characters and `\\` / `"` escaping, and has dedicated Rust tests for normal, escaped and UTF-8 names. This affects application launches, not WM hot-path idle memory.

### What was deliberately not optimized

- `tools/flamewm_desktop_services.py` is a long-lived Python companion. Replacing it could reduce resident baseline, but risks XDG autostart, notification/polkit lifetime and the four-agent session integration. Needs measured PSS + replacement ownership plan before removal.
- `crates/flamewm-integrations-linux/src/icons.rs` already bounds raster cache to 8 MiB by default. Reducing it without a miss-rate probe may increase filesystem I/O, allocations and visual latency.
- `crates/flamewm-render-x11` maintains retained X11 backbuffers; eliminating them can reintroduce flicker/tearing. `crates/flamewm-image-core/src/blur.rs` already shrinks/reclaims scratch buffers; no speculative change.
- `flamewm-shell/src/app.rs` clones snapshots during domain refresh. Removing these needs a borrow/ownership refactor crossing panel projections and is out of scope without native compilation.

## Preserved contracts

No change to application placement (Agent 01), launch-badge presentation/lifecycle (Agent 02), KDE wallet or Brave profiles/session integration (Agent 03), display topology, taskbar geometry or Settings monitor layout (Agent 04), XDG desktop services (Pass 32), UI assets, package dependencies or Cargo target definitions.

## Verification

- Native on-user-machine `cargo check --workspace --all-targets`: **reported PASS** in supplied log before packaging failed; this Pass 34 patch has **not** been compiled by Rust locally (no `cargo`/`rustc` in this container).
- Focused real Python packaging/manifests / Cargo helper migration / four-agent contract suites: **29 passed**.
- Additional Pass 27 / MIME / Cargo gate / Pass 34 suites: **17 passed** (4 Pass 34 tests overlap the above 29; do not treat this as 46 unique tests).
- `scripts/package` Debian fixture: built a real fixture `.deb` using stub executables and verified metadata, session compatibility installed paths, checksums and failure cleanup. **Not a native Rust application build**.
- Bash static syntax (`bash -n`) for relevant release and session integration scripts: PASS.
- The source archive built in a synthetic reproduction passes; complete source release archive generation in this sandbox is blocked because the overlays deliberately omit many `assets/` and `.vendor/` files from the **full** checkout. The end-user's release build must verify these locally.
- UI contract guard: 0 errors, 24 pre-existing warnings.
- Architecture guard remains red with six pre-existing/preserved boundary violations (including `flamewm-applications` X11 linkage); they are not introduced by Pass 34. This pass does not silence guard checks.

## Installation and verification on real repository

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS34_MEMORY_PACKAGING_FIXED_2026-10-20.tar.gz"
test -s "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && tar -xzf "$ARCHIVE" -C . && \
python3 -m unittest -v tools.test_flamewm_pass34_release_memory_contract && \
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

After `FLAMEWM-DEB-READY`, install the **newly built** `./.target/dist/flamewm_0.1.0_amd64.deb`. The source archive is a replacement for Pass 33, not an update to extract alongside older passes. Never re-extract Pass 27–33 afterward. The archive does not repair the user's pre-existing local dirty modifications; back up those before overlaying.

## Release risk / skipped

Rust compile and hardware-dependent testing, especially monitor rearrangement and Brave/KWallet unlocking, **not performed** in this sandbox. No PSS/RSS comparison, no claim of numerical runtime memory savings. The six architecture violations and the host-level KDE/portal behavior remain outside this pass.
