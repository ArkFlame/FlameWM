# Build timing report (2026-10-23/24)

All numbers measured on a shared 12-core machine at load 42-75. Treat as +/-2x.

| Workflow | Time | Warm/cold | Peak RSS | Exit |
|---|---|---|---|---|
| Production `scripts/build-deb`, release, CARGO_BUILD_JOBS=2 (previous build) | 31m00s | cold | n/a | 0 |
| Production `scripts/build-deb`, release, CARGO_BUILD_JOBS=2 (latest, compile only) | 42m12s | cold-ish (new target dir `flamewm-targets/deb`) | n/a | see log |
| Dev `.deb` `scripts/dev-deb` (profile `fast`) | 23m30s | cold | 681 MB | 0 |
| Dev `.deb`, one-file edit | 109 s, 149 s | warm | n/a | 0 |
| Dev `.deb`, one-file edit (earlier run) | 720 s | warm, cause untraced | 674 MB | 0 |
| Dev `.deb`, no change | 119 s | warm | n/a | 0 |
| `cargo build --profile fast --bin flamewm-terminal`, one-line edit | 8-12 s | warm | 269 MB | 0 |
| Same, after CSS change | 115 s | warm | 400 MB | 0 |

No pre-session baseline was measured for the dev loop, so no change-% is given.

## What costs the most
- Release uses `opt-level=3`, ThinLTO, `codegen-units=1` (unchanged, production semantics).
  Each crate is a single-threaded codegen unit, so the build is bound by the longest
  dependency chain and by `CARGO_BUILD_JOBS`.
- Latest build log shows the heaviest late crates: `flamewm-render-x11`, `flamewm-explorer`,
  `flamewm-desktop`, `flamewm-shell`, `flamewm-x11-canary`. No per-crate `--timings` was
  captured, so per-crate seconds are NOT measured.
- Parallelism was 2 jobs of 12 cores (script default). Changed to 10 in `scripts/build-deb`
  (line 66); override with `CARGO_BUILD_JOBS`. Expect memory to grow with jobs
  (release rustc ~0.7-0.8 GB each); effect on time is unmeasured, and the machine was
  already loaded by other users.

## Not done
opt-level 1 vs 2 comparison, idle-machine rerun, `--timings` profile, package-pipeline test
timeouts (`test_explicit_cargo_target_dir_is_created_and_used` failed once, undiagnosed).
