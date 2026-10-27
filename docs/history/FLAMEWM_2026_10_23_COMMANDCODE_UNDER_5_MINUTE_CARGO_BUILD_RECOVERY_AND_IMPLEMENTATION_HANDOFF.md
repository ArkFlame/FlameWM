# FlameWM: under-five-minute Cargo builds — source-grounded CommandCode implementation handoff

**Date:** 2026-10-23 | **Audience:** CommandCode / DeepSeek V4.1 coding agent operating inside the active FlameWM Rust repository | **Task:** implement, measure and iteratively improve build latency without sacrificing correctness, source readability, runtime behavior or production release optimization.

> **IMMEDIATE INSTRUCTION:** An existing `scripts/build-deb`/Cargo release compilation is RUNNING on the user's computer. **Do not interrupt, kill, renice, restart, replace, clean, or race it.** Begin with read-only inspection and disjoint source/tooling planning. Start performance experiments and change build knobs **only after that build and packaging process terminate normally**. Check the *actual exit/status*; a shell background command's exit 0 does not prove the inner `.deb` build passed. Preserve its target cache and log.
>
> **Concrete objective:** get the frequent development edit→compile→test→Xephyr and development `.deb` build workflows to **≤300 seconds on the user's hardware**, ideally far below this when warm. Also minimize production `.deb` rebuilds through correct cache reuse. **Do not claim that a clean full production release build with LTO can reach 300 seconds** until measured; it is a different workload. Preserve exact production release artifact semantics. If a measured workload remains over five minutes, identify its dominant critical path and continue improving rather than swapping one 40-minute blind build for another.

## 0. Context and evidentiary boundaries — read before modifying anything

This handoff was researched from `/mnt/data/1936_23_10_26.tar.gz` (source snapshot supplied 2026-10-23) and `/mnt/data/Pasted text(20261023-223552).txt` (Claude/agent transcript, tail through the 7:28 p.m. status). **The agent's live checkout is newer or otherwise dirty: it is the source of truth.** Snapshot files are a reference only. The archive lacks `.git`, `.cargo` and large runtime assets; do **not** extract it onto the working tree or replace user files. User checkout described in the transcript:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender
```

Reported latest facts from the transcript, not independently observed live:

- `cargo check --workspace --all-targets --locked` in `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/flamewm-targets/final` and `CARGO_BUILD_JOBS=4` completed in about **four minutes**, exit 0. A previous run took 6m16s. Warm scoped counter tests reported about **four seconds**; cold debug targets took ~594s. This is decisive evidence of cache and profile effects.
- `./scripts/build-deb` with `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/flamewm-targets/deb` was still compiling `cargo build --release --workspace` after about **20 minutes**, with two `rustc` processes and no observed compile error at the last report. The user now describes **over 40 minutes**; this is a wall-clock user report, not a completed measurement in the provided log.
- There have been many independent target roots (`final`, `deb`, `dev`, `fast`, `fast-bench`, and formerly `/tmp/claude-1000/target-*`). They do not generally share compiled artifact fingerprints. The prior long baseline experiment built separate release trees and was abandoned for excessive wait.
- Existing `tools/fast-bench.sh` was consolidated to a shared `fast-bench` target and warmed to ~4 seconds. Do not accidentally regress this.
- Agent is concurrently working on expensive X11/render CPU changes; preserve all dirty source, all artifacts, and accepted functionality. No exploratory full refactor of rendering is part of this handoff.

Inspect `AGENTS.md`, `.gitignore`, current instructions and other active handoffs in the **real** repo before implementation. Freeze `git status --porcelain=v1`, `git diff --stat`, source SHA, running build PIDs, log path and target dir. Other agents may be editing; coordinate ownership.

## 1. Confirmed source findings (from the supplied snapshot)

### 1.1 Root Cargo profile prioritizes peak runtime speed, not build speed

`Cargo.toml` (version `0.1.1`, Rust 2024, toolchain pinned to `1.85.1`) includes:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
panic = "abort"
strip = "symbols"

[profile.release.package.flamewm-render-compiler]
opt-level = 2

[profile.release.build-override]
opt-level = 2

[profile.fast]
inherits = "release"
lto = false
codegen-units = 16

[profile.dev]
debug = "line-tables-only"

[profile.dev.build-override]
opt-level = 2

[profile.dev.package."*"]
debug = 0
```

**Diagnosis:** `-C opt-level=3`, ThinLTO and `-C codegen-units=1` in the *release* profile can make compilation and the final code-generation/link stages far more expensive, particularly in a 41-crate workspace. Turning up Cargo jobs alone cannot parallelize a single crate's constrained LLVM codegen or final ThinLTO link. Existing `fast` removes ThinLTO and uses 16 codegen units, **but still inherits `opt-level=3` and the release default of `incremental=false`** unless overridden. The `dev` build-override uses `opt-level=2` for build dependencies; unlike Cargo's defaults, this increases compile effort for build scripts/proc-macros and may cause duplicate compilation of dependencies with different profiles.

**Do not modify release flags globally to meet a test-build SLA.** Keep unchanged production opt-level, ThinLTO, codegen units, panic and stripping unless independent runtime+binary benchmarks and explicit decision later authorize a production tradeoff.

### 1.2 Debian pipeline always triggers a full release-profile workspace build

`./scripts/build-deb`:

```bash
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/.target}"
if [[ ! -w "$TARGET_DIR" ]]; then TARGET_DIR="$ROOT/.target"; fi
export CARGO_TARGET_DIR="$TARGET_DIR"
# checks tools, architecture, assets and packaging/source-manifest.txt
python3 -I scripts/package-source-archive --check "$ROOT" packaging/source-manifest.txt
python3 -I tools/ensure_desktop_conversion_cargo.py --profile release --include-ui
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build --release --workspace
./scripts/package
```

- Default job count **2** only when `CARGO_BUILD_JOBS` is unset. If the parent sets `CARGO_BUILD_JOBS=6`, **the script honors 6**; don't falsely report it always forces 2.
- **Dangerous silent fallback:** if the externally specified target directory does **not already exist and is writable**, the script falls back to `$ROOT/.target`. This can unexpectedly defeat artifact reuse or fill the source disk. Make target creation explicit and fail clearly on inaccessible configured target paths; never silently switch caches for a release job.
- `--workspace` builds default library/bin targets for all 41 crates; packaging actually requires a smaller list of 12 named binaries (below), while many libraries are needed transitively. Targeting exact package/bin owners may improve critical path and avoids unrelated workspace members. **Only change scope after showing equivalent executable coverage and tests; avoid stale executable acceptance.**
- `scripts/package` hardcodes `BUILD_DIR="$TARGET_DIR/release"`. A Cargo `--profile fast` build places executables under `target/fast/` and **cannot safely be fed to `scripts/package` without explicit profile-aware staging**.
- `scripts/build-deb` erases `$ROOT/.target/dist` first and validates output; `scripts/package` creates deterministic source archive, Linux tarball, .deb, checksums. These fail-closed protections must remain.

Production package-required binaries (`scripts/package`): `flamewm`, `flamewm-session`, `flamewm-desktop`, `flamewm-shell`, `flamewm-settings`, `flamewm-screenshot`, `flamewm-explorer`, `flamewm-render-x11`, `flamewm-render-compile`, `flamewm-render-inspect`, `flamewm-lock`, `flamewm-terminal`. **Count: 12 binary names; verify against the live checkout.** Do not silently omit Terminal or assets.

### 1.3 A targeted cache-corruption guard remains active

`tools/ensure_desktop_conversion_cargo.py` computes content SHA-256 for `flamewm-desktop-core` source files and the desktop RWR input set; validates actual Cargo metadata path dependency; stores digests per *resolved target directory*; and calls `cargo clean -p flamewm-desktop-core` and/or `cargo clean -p flamewm-desktop` only when digest state changed / marker missing / forced. Importantly, **the earlier unconditional `--force` issue is already fixed in this snapshot**. Do not remove or bypass this guard. It exists because source archives with old timestamps can fool Cargo's mtime-based freshness checks.

Important optimization hypothesis: even on the first build of a NEW cache with no existing `rlib`, a missing digest marker can trigger unnecessary targeted cleans; and any change to UI input forces desktop clean, invalidating recompilation of dependents. Investigate carefully and preserve safety with regression tests. A no-clean decision requires proof the target has never produced stale outputs, not just a convenient missing marker. Also `--profile fast` is not supported by this Python guard (`dev`, `release` choices only), so ensure a fast-build lane has equivalent cache safety and **a correct mapping from Cargo profile to artifact directory/clean flags**.

### 1.4 Existing fast development workflow is good but incomplete

`scripts/dev-lane` already provides:

```bash
scripts/dev-lane check <crate>...   # shared .../dev cargo check --all-targets
scripts/dev-lane test  <crate>...   # shared .../dev cargo test
scripts/dev-lane fast  <crate>...   # shared .../fast cargo build --profile fast
```

And `tools/fast-bench.sh` reuses one separate `fast-bench` cache across four microbench suites. Preserve both and unify or rationalize compatible caches **only when compiler mode/profile flags match**. Do not blindly combine incremental debug/tests and release builds in ways that cause eviction or contention. Different modes create separate `debug/`, `release/`, and `fast/` subtrees under a common root, so using a consistent root can still improve library and metadata reuse **within each mode**, without expecting cross-profile reuse.

### 1.5 Further build-time amplifiers to investigate rather than assume

- Workspace has 41 member crates; some large files include `flamewm-settings/src/lib.rs` (~168 KB), `flamewm-explorer/src/main.rs` (~161 KB), `flamewm-shell/src/runtime.rs` (~154 KB). Large files do **not** by themselves prove compiler hotspots; look at Cargo timing and compiler self-profiles.
- `scripts/xephyr` normally builds six or more binaries with `--release`; use a faster profile for development Xephyr only if smoke-test fidelity and execution path remain correct, reserving full release for certification.
- `scripts/check` does full workspace check then release workspace build then `cargo run --release` compiler/inspector. This may be expensive on every use; keep full gate for final verification and create fast scoped feedback paths.
- Cargo's final link may dominate when compile cached. Test `mold`/`lld` only if present and version-compatible and do not casually set global `RUSTFLAGS` (changing flags changes fingerprints and induces a one-time rebuild).
- `sccache` helps matching non-incremental rustc compilation after invalidations or across cache roots, but **does not generally cache incremental Rust compilations or final binaries**. Trial separately and report hit rates; don't bundle it with incremental builds as if both stack automatically.
- Debian/source archive compression or copying icon assets may be costly independently from compilation; measure `cargo build` and `scripts/package` wall-clock **separately**.

## 2. First action when this prompt is received (while user's build is still running)

**Read-only commands only; do not begin a second Cargo build:** 

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
pwd -P
git status --porcelain=v1
git diff --stat
rustc -Vv
cargo -V
nproc
printf '%s\n' '--- running compile/package processes ---'
pgrep -af 'cargo|rustc|cc1|clang|ld.lld|mold|ld.gold|lto|dpkg-deb' || true
printf '%s\n' '--- memory/swap ---'
free -h
cat /proc/pressure/memory 2>/dev/null || true
printf '%s\n' '--- disks and existing targets ---'
df -h /run/media/linsaftw/SDD "$PWD"
du -sh /run/media/linsaftw/SDD/flamewm-targets/* 2>/dev/null | sort -h | tail -15
printf '%s\n' '--- recent log (read only) ---'
tail -n 35 /run/media/linsaftw/SDD/flamewm-targets/deb-build.log 2>/dev/null || true
```

Use `ps -eo pid,ppid,etime,stat,%cpu,%mem,rss,cmd --sort=-%cpu | head -35` for current process data. If permitted, inspect cgroup memory (`/sys/fs/cgroup/memory.current`, `memory.max`, `memory.events`), thermal throttling (`sensors` if installed), blocked I/O (`iostat -xz 1 3`), memory pressure, swap-ins, current clocks and disk read/write throughput. **Do not assume low CPU means Cargo is hung: a single long ThinLTO/codegen/link task may be on the critical path.**

Do **not** modify the running command's `CARGO_BUILD_JOBS` mid-flight; it does not reconfigure the active jobserver. Do not create parallel Cargo builds or source edits in files the running build is reading. Run read-only explorers concurrently and stage potential safe changes for when the build completes. Coordinate with the other agent actively optimizing the renderer; this is a build-system-only handoff.

## 3. Critical policy: testable success, no misleading '5-minute' claim

Define distinct metrics and show all in final report, with exact command, target dir, profile, compiler and exit code:

1. **WARM scoped check/test** after changing one normal Rust module: end-to-end wall time, goal as low as practicable (seconds to 1 minute; certainly <5min).
2. **WARM fast Xephyr/test package** after typical shell/WM/Desktop changes: goal <5min, including packaging if claimed.
3. **WARM full production `.deb` rebuild** with no changes: aim <5min through cache reuse and fast packaging. If Cargo is fresh, still validate publication and checksums.
4. **INCREMENTAL production `.deb`** after one changed leaf crate: aspire <5min; report when ThinLTO or critical dependencies make it longer.
5. **COLD production release `.deb`** from a new empty target: report separately; not eligible to be labeled a <5min success without real proof.

A complete standalone under-five-minute build may not be feasible on this laptop. Do not fake a cache hit, skip compilation of changed code, or call a `cargo check` a runnable or release build. Every measured fast path must produce and validate the requested artifacts. Report confidence, repeat count, and high-water RSS/swap. Prefer one representative warm run after each optimization batch over rebuilding the entire workspace repeatedly.

## 4. Execution plan — FULL READY-FRONTIER SATURATION + one Cargo owner

Coordinator is **control plane only**. It does not directly edit, build, run display probes, or execute Git operations when agents/subagent tooling exists. The coordinator maintains DAG, owners, evidence and decisions; all repo operations happen in bounded agents. Distinct agents may read different source segments concurrently, but **one shared Cargo build/test/link owner at a time**; compiling in separate target directories on the same constrained hardware can be *slower*, not faster. Use asynchronous background agents for research, code modifications, regression tests, policy and documentation, **not competing Cargo jobs**. Refill the READY frontier promptly as agents finish; cap active writers to verified disjoint files and avoid more than CPU/memory allows. Independent verifier (fresh agent) alone may declare `PASS`; builders return `PATCH_APPLIED_NEEDS_VERIFY`, `BLOCKED` or `FAILED`.

**Wave A — during existing build (read-only, launch in parallel):**

- **A1 source pipeline:** ownership `scripts/build-deb`, `scripts/package`, `scripts/xephyr`, `scripts/check`; map actual command graph, artifacts, profile assumptions, package parity, safe change scope.
- **A2 cache identity:** ownership read-only `tools/ensure_desktop_conversion_cargo.py`, its tests and all build target dirs; find hidden clean triggers, target fallbacks, fingerprint misses, timestamp hazards.
- **A3 cost profile and hardware:** read-only parse active log, `ps`, `/proc`, RAM/swap, CPU quotas, disk pressure, Cargo flags and rustc invocations. Locate current long-running critical unit; do not run another compiler.
- **A4 dependency and linker:** read-only Cargo.toml files and lockfile, detect `lto`, `build.rs`, proc-macro duplicates, binary target owners, linker availability; prioritize measured suspects only.
- **A5 verification plan:** read-only tests, source-manifest, 12 binary names, package conffiles and expected hashes; design isolated fast-lane acceptance.

**After existing build finishes and exact exit is recorded, implement disjoint areas concurrently:**

- **B1 build-lane owner:** `scripts/build-deb` plus `scripts/dev-lane` + any new dedicated opt-in wrapper, no edits to `scripts/package` while B2 owns it. Add explicit target-dir handling, job tuning, `--locked` in proven safe contexts, clear logging/timings and a safe fast-deb entrypoint. Preserve existing full-release CLI.
- **B2 packaging owner:** `scripts/package` and tests: explicit build profile/staging and distinct distribution path if fast `.deb` is needed. A fast test package must be unmistakably labeled and cannot overwrite a verified production file.
- **B3 Cargo profile owner:** root `Cargo.toml` and profile tests/docs: separate compile-optimized `fast` settings, build-dependency overrides only if shown worthwhile. Test any profile choice against expected runtime functionality and avoid meaningless code churn.
- **B4 cache guard owner:** `tools/ensure_desktop_conversion_cargo.py` and its tests: profile-agnostic content correctness, minimal invalidation, per-target identity; do not accidentally remove stale-RWR protection.
- **B5 tests/docs owner:** packaging acceptance/fixtures, `tools/test_dev_lane.py`, `tools/test_flamewm_package_pipeline.py`, runtime smoke scripts, docs and benchmarking harness. Update source-manifest for any new executable scripts before packaging. Reuse existing test utilities.

File ownership is exclusive, never let two agents edit `Cargo.toml`, `scripts/build-deb`, `scripts/package` or the same Python tests simultaneously. Cross-owner API changes are proposed in a compact contract and integrated in one verification gate. **Wave C:** one Cargo owner runs short focused tests sequentially (can batch multiple packages in one cargo invocation). **Wave D:** if red, launch disjoint repair agents; rerun the identical failure gate before broader tests. **Wave E:** independent verifier checks production unchanged and validates packages; only then promote to PASS.

Don't launch a fresh whole-workspace full release after every change. Batch 3–5 safely independent build-system patches, run focused tests, then one clean integration gate. When a failure repeats, first construct the exact regression test/probe that fails, repair first canonical owner, rerun the identical probe; never stack speculative tweaks.

## 5. Implementable optimizations, in priority order

### P0: remove cache-root mistakes and needless releaselike work

1. Make one **stable target base/root** for normal development. Example policy: `FLAMEWM_TARGET_BASE=/run/media/linsaftw/SDD/flamewm-targets`, with a stable development root and a stable production root if needed, always consistently reused. Profile directories are already isolated within a root; do not create per-agent cache roots by default.
2. In `scripts/build-deb`, if `CARGO_TARGET_DIR` is explicitly set, create it with `mkdir -p` if allowed; if not writable, **fail with a descriptive error** rather than silently falling back to `.target`. If not set, choose the documented default intentionally. Preserve user-supplied paths; do not overwrite permissions or delete caches.
3. Deduplicate redundant full `--workspace` builds in the normal edit loop. Use crate-local test/check for incremental changes; full production build only at release gates. The user specifically needs *faster feedback* and *reusable packages*, not a superficial shell wrapper that still does 40 minutes of release compilation.
4. Stop running duplicate Cargo builds concurrently. Audit background tasks that spawn their own target dirs while another process compiles. Detect and serialize only build stages, not source inspection/editing.
5. Validate source manifests/assets **before** compilation (already implemented; preserve and improve failure messages). Avoid wasting 40 minutes only to fail `scripts/package` over a missing executable or manifest line.

### P0: actually make the development Debian lane fast

The existing `[profile.fast]` is the natural starting point. **Illustrative hypothesis** (agent must benchmark and adjust values, keeping production untouched):

```toml
# Existing profile. Make it a consciously faster runnable development lane.
[profile.fast]
inherits = "release"
opt-level = 1           # experiment with 1, then 2; NOT guaranteed runtime parity
lto = false
codegen-units = 16
incremental = true      # workspace/path crate recompiles; verify disk cost

# If build.rs / proc macro compilation was actually a bottleneck:
[profile.fast.build-override]
opt-level = 0
codegen-units = 256
```

`opt-level=1` may be too slow during visual FPS/CPU tests. In that case use opt-level 2, or keep the existing fast profile and add a `fastest` or `dev-deb` profile. **Do not benchmark FlameWM runtime performance with a low-opt build and compare to the release baseline as if equivalent.** This lane is for rapid functional smoke tests and iterative development. Add explicit `scripts/build-deb --fast` or `scripts/build-deb-dev` **only if** it correctly selects the fast profile, passes exact profile into the packaging staging logic, uses matching cache gate, and generates distinctly named `~dev` or similarly clearly non-production artifacts. Prefer one existing script with parameterized profile over duplicated 4 KB scripts.

The packaging implementation must provide:

- `BUILD_DIR` is chosen explicitly from `release` versus `fast` using a whitelisted profile value; never consume stale `release/` binaries after compiling `fast/`.
- Exact 12 required executables are built from the same checkout and all must exist and be executable. A fast build should not be silently installed over the production OS.
- Separate distribution output (e.g. `.target/dev-dist` and a clearly differentiated Debian package version suffix), checksums and a non-production warning. Do not change the usual `./scripts/build-deb` production semantics.
- Identical package structure, assets, desktop entries, security/session startup and native UI; test installed layout on an isolated path with `dpkg-deb --contents`, not live install.
- No `sudo apt install` or invasive system updates on the user's machine unless explicitly asked.

### P1: optimize release recompilation WITHOUT compromising final runtime output

Keep the existing `[profile.release]` untouched. Improve package selection, cache identity and repeated-build efficiency first. Potential Cargo command when parity is established:

```bash
# EXAMPLE ONLY — no changes to production yet; package names must be verified.
cargo build --release --locked --timings \
  -p flamewm-wm -p flamewm-desktop -p flamewm-shell -p flamewm-settings \
  -p flamewm-screenshot -p flamewm-explorer -p flamewm-render-x11 \
  -p flamewm-render-compiler -p flamewm-lock -p flamewm-terminal
```

Every required binary must be newly present in the selected target profile and attributable to the current fingerprint; if a partial package selection could accidentally reuse a stale executable, fail closed or keep `--workspace`. Confirm `flamewm` and `flamewm-session` are both built from `flamewm-wm`, and `flamewm-render-compile` plus `flamewm-render-inspect` from `flamewm-render-compiler`.

A leaf-crate edit should not trigger rebuilding unrelated high-level crates unless they really depend on the changed crate. Use `cargo tree -i`/`cargo metadata` to map reverse deps; do not manually imitate internal Cargo fingerprint logic or invent a less-safe custom incremental compiler. Cargo *already* rebuilds only changed units/dependents **within a stable cache/profile/flags setup**. The job is to stop defeating Cargo's existing system.

### P1: measure and tune job count, under actual RAM/swap constraints

After current build finishes, agent computes:

```bash
nproc
free -h
cat /proc/pressure/memory 2>/dev/null || true
cat /sys/fs/cgroup/memory.max 2>/dev/null || true
cat /sys/fs/cgroup/memory.events 2>/dev/null || true
```

Start with `CARGO_BUILD_JOBS=4`, then test `6` and optionally `8` on reasonably comparable warmed/cold workloads **only when adequate free RAM**, minimal memory stalls and no other Cargo process. Do not blindly use `-j$(nproc)` on a laptop already swapping. A multi-rustc build can lose performance sharply to OOM, page faults, swap or thermal throttling. Use `--timings` to determine whether cargo actually reaches job concurrency; when only one long crate/codegen/LTO task remains, increasing Cargo jobs will not help.

Example safe invocation after current job completes:

```bash
export CARGO_TARGET_DIR=/run/media/linsaftw/SDD/flamewm-targets/deb
mkdir -p "$CARGO_TARGET_DIR"
CARGO_BUILD_JOBS=6 /usr/bin/time -v \
  cargo build --release --workspace --locked --timings
```

**This is a measurement command, NOT an instruction to repeatedly do expensive cold builds.** If the current release has finished, warm builds should reuse it; take one timed warm build with actual exit, not an unnecessary second fresh target dir. `cargo --timings` writes HTML to the target's `cargo-timings/`; read the actual longest unit and critical path.

### P1: link speedup, optional and reversible

Inspect `command -v mold ld.lld clang cc`, compiler version, build artifacts, `--timings` final units. Test alternative linker on the **fast/test lane first**, not global release. Example local/opt-in config after validation (as Cargo documents):

```toml
# opt-in only, and verify that a supported compiler driver invokes mold
[target.'cfg(target_os = "linux")']
# rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

An actual project `.cargo/config.toml` change affects all Linux builds and may trigger a full rebuild; avoid by using a dedicated wrapper/environment for the experiment. If using `clang`/`lld`, verify correct startup/linker flags and binary runtime. Measure `mold`, `lld`, default separately. Roll back absent a meaningful time gain or on runtime/package failure.

### P2: compiler cache only if it has measured utility

```bash
# opt-in non-incremental compilation experiment; no blind global export
command -v sccache
RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0 cargo build --profile fast -p flamewm-shell --locked
sccache --show-stats
```

Do not use this exact command as production automation without trial. With Rust, `sccache` cannot effectively cache rustc's incremental compilation and cannot reuse linking-heavy binary/proc-macro outputs in the same way as cacheable rlibs. Changing to sccache and disabling incremental may make *local incremental edits* slower. It may help when repeated clean dep builds occur. Accept only when hit rates and latency justify it. Do not upload private source artifacts to a remote/shared cache without user consent.

### P2: profile/build-script/dependency work if timing says it matters

- Profile `flamewm-render-compiler`, `flamewm-desktop/build.rs` and other build.rs custom builds in `cargo-timing.html`; avoid rebuilding UI generated assets for unrelated Rust code when input bytes unchanged. Keep complete dependency tracking (`cargo:rerun-if-changed`), stable deterministic outputs and old-timestamp archive safeguards.
- Evaluate `opt-level=2` in `[profile.dev.build-override]` and `[profile.release.build-override]`: trial `0` where safe; check if slower build script execution outweighs faster compilation, and watch duplicate dependency variants. Report which one wins.
- Use `cargo tree --duplicates` and `cargo tree -e features` to identify expensive duplicate versions/features. Change dependencies/features **only** when their compile-time impact is measured and behavior tests cover removed capabilities. Do not remove modules, split unrelated big files, or rewrite framework owners merely for speculative build gains.
- Treat nightly `-Zthreads`, Cranelift, unstable compiler options, alternative allocator/linker, PGO and compiler upgrades as separately gated R&D. Root toolchain is pinned to stable 1.85.1; **no implicit nightly switch or toolchain migration**.

## 6. Measurements and test harness: efficient, fair and reproducible

Create or extend a small `tools/bench-cargo-build.sh` (or Python if needed) that:

- Logs exact wall time, max RSS, user/sys CPU, swap-in/out, exit code, Rust/Cargo version, profile, jobs, target dir, command, dirty source/flag identity and Cargo timing report location.
- Detects active Cargo/rustc processes before starting; fails closed or explicitly coordinates with the single build owner (do not kill them).
- Differentiates warm no-op builds, incremental Rust module edit builds, fresh fast profile, development package and full production release; **never edit source files as a benchmarking trick without robust restoration**. Use controlled test fixture or a disposable copy when needed; never reset WIP.
- Does not accidentally read old build logs and report them as current runs; each run has a unique timestamp/ID and atomic summary file.
- Uses one stable cache root for comparable tests; changing jobs is benchmarked on distinct but comparably warm runs when possible, with caveats. Include 2–3 samples for warm, one cold sample only if necessary and approved by execution budget.
- Separates compilation and packaging times, e.g. `cargo build` exit and `scripts/package` exit.
- Runs `cargo --timings` only on meaningful integration/full builds; no enormous profiler workflow if scoped tests can prove the requested mechanism.

Useful diagnostic commands:

```bash
# Identify additional build scripts and proc macros
rg -n 'build\s*=|\[build-dependencies\]|proc-macro|codegen-units|lto\s*=|opt-level\s*=' Cargo.toml crates/*/Cargo.toml
cargo tree --duplicates

# Show Cargo's actual target root in the real checkout
cargo metadata --no-deps --format-version 1 | python3 -c \
  'import json,sys; x=json.load(sys.stdin); print(x["target_directory"])'

# Capture fingerprints only if an unexplained rebuild persists (may be noisy)
CARGO_LOG=cargo::core::compiler::fingerprint=info \
  cargo build --profile fast -p flamewm-shell --locked \
  2> /tmp/flamewm-cargo-fingerprint.log
```

Beware that `rg` and `cargo tree` are source/dependency inspection commands, while `cargo build` is an execution gate; separate owners accordingly. For hash correctness reproduce the pre-existing regression tests and add one that changes RWR input bytes while preserving older mtime: package must rebuild and embed new content, then a second unchanged run must skip invalidation. Also test unrelated source touch, missing marker with empty artifacts, marker from another target, corrupt marker, interrupted build and profile switching.

## 7. Acceptance gates — do not weaken or skip them

**G0 existing run preserved:** terminal build was not killed; real exit code and artifacts/log captured. Any failed package remains non-installable and cannot be confused with old `.deb`.

**G1 stable cache:** target dir is visibly fixed across invocations, no fallback surprise; no second rustc tree unnecessarily building while primary build runs; `cargo clean` only for provably stale known artifacts.

**G2 true targeted feedback:** selected crate check/test and `tools/fast-bench.sh` operate with a reusable warm cache, produce correct outputs, capture exit 0. Build owner verifies upstream reverse dependencies when their public API changes.

**G3 development fast-deb:** a dedicated fast package built from current changes, with exact 12 binaries, correct metadata, source and checksums. Unlike production, it must be unambiguously marked test-only; actual wall time under **300s** on warm representative edits, or report the remaining bottleneck and keep iterating. Smoke-test in isolated Xephyr/Xvfb with current scripts and visual checks. Do not claim a screenshot was taken when it wasn't.

**G4 production unchanged:** original `./scripts/build-deb` still uses `cargo build --release` with opt=3, ThinLTO, codegen-units=1, matching runtime behavior/ABI and all package contents. No compiler flags or unintended dependency feature changes. Verify package using existing `tools/test_flamewm_package_pipeline.py`, `tools/test_flamewm_pass25_cargo_recovery.py`, `tools/test_desktop_conversion_cargo_gate.py`, `tools/test_dev_lane.py`, `tools/test_fast_bench_lane.py` and related tests discovered in real checkout. Update acceptance expectations consciously if introducing optional args or distinct fast artifacts.

**G5 changed source compiled:** no package can pass just because stale executables happen to exist. Verify profile/paths, per-bin presence, changed input embedded in output where required, and content-aware desktop cache contract (including old mtime case). The custom gate must remain effective in both release and fast lanes.

**G6 measured results:** record before/after cold/warm/fast-deb/full-release stages without fabricating timing savings. Independently report build CPU, wall-clock, disk usage and peak RSS/swap. If job count 8 worsens time, revert to 4/6. If `sccache` misses, disable it. If any new flag changes production runtime performance or visual fidelity, remove it from release and keep it opt-in only.

**G7 no architectural damage:** minimal composable scripts and one canonical build/profile owner. No massive cloned build scripts, no fragile source file touch hacks, no rewriting Cargo's dependency graph, no generated code checked in unnecessarily. Preserve readability and source-manifest registration for new executable tools. No changes to native session, KWallet, compositor, rendering, or user app data.

**G8 independent verifier** repeats critical tests, inspects diff for WIP loss and checks packaged binary/app entries. A builder's “compiles” or “done” is insufficient for PASS.

## 8. Autonomous stop/continuation law

Proceed autonomously through the complete READY DAG. If a task is blocked by the live build, do useful read-only analysis and prepare independent edits elsewhere, then resume after the build's completion. Do not spin on the same compile command, recompile identical dependencies in fresh target dirs, or ask for permission after every small safe change. After each batch, measure the exact mechanism changed. A regression requires reproducer → first canonical fix → identical regression pass. Do not hide failed tests, skip meaningful visual checks, or claim the goal when only `cargo check` is fast.

Deliver a final concise table: **workflow, old time, new time, change %, warm/cold, peak memory, exit status, verification**. Include changed files, accepted/rejected experiments, any unverified or impossible SLA, and exact follow-up action if the 300-second goal is not yet met. Continue searching reasonable changes until measured development build target is met or a technically supported bound is proven. Never trade safety or release runtime optimization merely to hit a timing number.

## 9. Research references (official, current guidance; implementation details must be checked against pinned Cargo 1.85.1)

1. Rust Cargo Book — [Optimizing Build Performance](https://doc.rust-lang.org/cargo/guide/build-performance.html): debug info, alternative linkers, workspace dependency organization; emphasizes measuring tradeoffs.
2. Rust Cargo Book — [Build Timings](https://doc.rust-lang.org/cargo/reference/timings.html): `--timings`, build-critical path, codegen, build-script unit duration, active/waiting graph.
3. Rust Cargo Book — [Profiles](https://doc.rust-lang.org/cargo/reference/profiles.html): `opt-level`, ThinLTO, `codegen-units`, `incremental`, release/dev defaults, build-override tradeoffs.
4. Rust Cargo Book — [Build Cache](https://doc.rust-lang.org/cargo/reference/build-cache.html): target directory, per-profile output subdirectories, host/target separation.
5. Rust Cargo Book — [cargo build](https://doc.rust-lang.org/cargo/commands/cargo-build.html): `-p`, `--bin`, `--profile`, `--locked`, `--timings` and job count behavior.
6. Mozilla — [sccache Rust caching limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md): incremental compile incompatibility and unsupported linker-produced targets.
7. Rust Cargo Book — [Configuration](https://doc.rust-lang.org/cargo/reference/config.html): profile environment overrides and linker config, noting newer Cargo options may not exist on 1.85.1.

**Source-grounded vs hypothetical:** The profile values, build scripts, cache guard, target naming and executable list above were directly inspected in the supplied October 23 archive. Reported timings and the still-running compilation came from the uploaded transcript/user. Linker speedups, alternate profiles, safe package selection and `sccache` benefits are *candidate experiments*, not measured FlameWM improvements. The agent must check the live source and obtain evidence before adoption.

---

### Final instruction to CommandCode

**Begin now. Wait for the currently running build to complete normally while read-only agents investigate. Then apply the highest-payoff *actual* compilation-speed fixes, choose optimal Cargo jobs based on memory and critical path, make the development Debian lane <5 minutes with proper profile-aware staging, preserve full-production release and cache validity, test independently, and iteratively refine. Keep working until the verified objective is satisfied or quantitatively explain the irreducible blocker.**
