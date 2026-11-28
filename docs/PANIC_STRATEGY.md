# Panic strategy: `panic = "abort"` vs `diagnostics::guard`

Status: decision input, not a decision. No Cargo.toml change is made by this document.
Unit: `docs/swarm/pool/p88-panic-abort-decision.md`. Nothing here is built or measured yet.

## 1. Facts

### 1.1 The conflict

- `Cargo.toml:66-71` sets `[profile.release] panic = "abort"` (with `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`).
- `crates/flamewm-session-core/src/diagnostics/failure.rs:273-293` defines `guard()`, which wraps the operation in `std::panic::catch_unwind`.
- Under `panic = "abort"`, `catch_unwind` never returns `Err`. A panic runs the panic hook and then aborts the process with SIGABRT. So every `guard()` is a pass-through in release builds: it only adds the subsystem-name thread-local bookkeeping.
- The panic hook (`install_panic_hook`, `failure.rs:250-267`) still runs before the abort. It appends one `FailureRecord` with code `panic` to the session log and then calls the previous hook. So a release panic is logged, then the process dies.
- Unit tests of `guard` (`failure.rs:425-453`) pass only because `cargo test` uses the default `unwind` strategy. The tests do not exercise the release configuration.
- Reference precedent in `.vendor/` (read-only evidence): COSMIC release profiles (`cosmic-settings`, `cosmic-applets`, `cosmic-osd`, `cosmic-screenshot`, `pop-launcher`) set `panic = "abort"`, so the shipped-abort posture matches desktop peers; `catch_unwind` appears there in helper/library code, not as a supervisor-level recovery path. No KDE, xfce or smithay corpus is present in `.vendor/`.

### 1.2 Call sites of `guard`

| Site | Subsystem label | What it protects |
| --- | --- | --- |
| `crates/flamewm-lock/src/run.rs:346` | `frame-pacer` | Frame-pacing work in the lock screen. |
| `crates/flamewm-desktop/src/input.rs:62` | `desktop-paste` | Clipboard paste handling. |
| `crates/flamewm-desktop/src/input.rs:104` | `desktop-drop` | Drag-and-drop handling. |
| `crates/flamewm-desktop/src/input.rs:233` | `desktop-tool` | Desktop tool action. |
| `crates/flamewm-desktop/src/files.rs:145` | `desktop-trash` | Move-to-trash file operation. |
| `crates/flamewm-desktop/src/wallpaper_loader.rs:179` | `wallpaper` | Background wallpaper decode/load. |
| `crates/flamewm-shell/src/main.rs:874` | `shell-settings` | Shell settings application. |
| `crates/flamewm-shell/src/icon_loader.rs:282` | `shell-icons` | Icon resolve/load work. |
| `crates/flamewm-wm-wayland/src/main.rs:776-777` | `guard_startup(subsystem, ...)` | Wrapper used for WM startup steps; it forwards to `guard`. |

Test-only call sites: `failure.rs:427, 433, 445-447`.

Intent of every production site: a panic in an optional feature (paste, drop, trash, wallpaper, icons, settings, pacing) should drop only that feature and keep the process alive. Under abort, that intent is not met: any of these panics kills the whole component.

### 1.3 Session supervisor (what happens when a child dies)

Source: `crates/flamewm-wm/src/bin/flamewm-session.rs` and `crates/flamewm-wm/src/bin/flamewm-session/supervision.rs`. Roles by index: 0 = window manager, 1 = desktop, 2 = shell.

- Policy: `SupervisorPolicy::default()` in `crates/flamewm-session-core/src/lib.rs:599-607`: `maximum_restarts = 5`, `restart_window_ms = 60_000`, `initial_backoff_ms = 500`, `maximum_backoff_ms = 8_000`, `graceful_stop_ms = 1_500`.
- Backoff per restart: `initial * 2^min(attempt, 7)`, capped at 8 000 ms (`lib.rs:704-710`). Sequence: 500, 1000, 2000, 4000, 8000, 8000 ms.
- Budget: restarts counted in a rolling window that starts at the first exit (`lib.rs:676-696`). The 6th exit inside 60 s gives up.
- `supervise()` (`supervision.rs:161-283`) reacts to any child exit, including SIGABRT from a release panic, because `status_to_exit` maps the exit status and `child_exited` does not distinguish panic from other exits. Clean WM exit is the only exit that stops the session.
- Respawn behaviour per role:
  - WM (index 0): stops dependents, waits backoff, respawns, waits for readiness, then restarts desktop and shell after WM readiness. On budget exhaustion or readiness failure it starts the fallback WM plan.
  - Desktop (index 1): waits backoff, respawns, runs startup grace; if the shell was not yet started it starts it. On budget exhaustion it starts the fallback desktop plan.
  - Shell (index 2): same respawn path with its own backoff and budget. On budget exhaustion it starts the fallback shell plan.
- Each exit writes a diagnostic log line and, on give-up or respawn failure, a `FailureRecord` (`abort_recovery`, `supervision.rs:17-47`).
- Gap: the supervisor logs only the exit code/signal. It does not read the panic record, so the abort path depends on the in-process hook having written first.

So today a release panic in any guarded feature takes down the whole shell or desktop process, which the session supervisor then respawns with the backoff above. The `guard` calls add no protection in release.

## 2. Options

### Option A: keep `panic = "abort"`, rely on process respawn

- Behaviour: a panic kills the component; the supervisor respawns it (500 ms up to 8 s backoff, 5 restarts per minute, then fallback).
- Remove or document the `guard` calls as dead code in release, or keep them for tests and debug builds only.
- Pro: smallest binaries and lowest idle memory; the default release profile is unchanged; this matches the current measured behaviour.
- Con: a panic in an optional feature (wallpaper, icons, paste) restarts the whole desktop or shell and drops its window state. The intent of `guard` is lost.
- Needed to make this honest: update the `guard` docs to say it is a no-op under `panic = "abort"`, and add a release-mode test or assertion that fails if the profile changes.

### Option B: `panic = "unwind"` only for specific binaries

- Behaviour: `guard` works in desktop and shell (and any other chosen bin); the WM keeps abort.
- Mechanism to check before relying on it: Cargo applies `panic` per profile, not per package. The `[profile.release.package.<name>]` overrides at `Cargo.toml:73-120` may not honour `panic`. Verify against the cargo version in use before choosing this. If not supported, Option B needs a separate custom profile (for example `[profile.release-unwind]` inherited from `release`) and a build path that selects it. Gate and packaging scripts would then need to change, which is out of scope for this unit.
- Pro: keeps the feature-level isolation where it matters most.
- Con: unwind tables and landing pads add size and some runtime cost; two release profiles to keep in sync; mixed strategy across the process graph.

### Option C: `panic = "unwind"` everywhere

- Behaviour: every `guard` works; panics in any component are caught.
- Pro: one strategy; `guard` means what it says.
- Con: largest binaries and highest idle RSS across all shipped binaries; the WM would then keep running after a panic in a state it may not be able to recover from, which is a design risk for the compositor itself.

## 3. Measurement to take (not yet taken)

Run only after the decision is chosen to test Option B or C. Do not run as a gate.

1. Binary size, for the release profile as-is versus `panic = "unwind"`:
   - Build `flamewm-shell` and `flamewm-desktop` release binaries once per variant through the normal release build path. Use the scheduler or the repo's release script; do not invent a new build command.
   - Record `stat -c '%s' <binary>` for each. Expect the unwind variant to be larger; the question is by how much.
   - Also record the total of `.text`, `.eh_frame` and `.gcc_except_table` (`size -A <binary>`).
2. Idle RSS for shell and desktop, same two variants:
   - Start the session in the normal way on the test setup with no user interaction for a fixed window (for example 30 s after the first frame).
   - Read `VmRSS` and `RssAnon` from `/proc/<pid>/status` at 10 s and 30 s. Use `scripts/proc-smaps-top` if it covers the process; do not add an ad-hoc script.
   - Report the median of three runs per variant.
3. Acceptance for choosing B or C: the binary-size delta and RSS delta are reported as numbers with the variant names; no numeric threshold is set here. Choose after reading the numbers.

### 3.1 Tool: `scripts/measure-panic-profile`

The script builds nothing. Build each variant through the normal release path first, then:

- `scripts/measure-panic-profile dirs <abort_target_dir> <unwind_target_dir>` prints, for every `flamewm-*` binary in `<dir>/release`, the abort bytes, unwind bytes and delta.
- `scripts/measure-panic-profile size <variant>=<binary> ...` prints file bytes and the `.text + .eh_frame + .gcc_except_table` total from `size -A`.
- `scripts/measure-panic-profile rss <variant> 10 30 -- <command> [args...]` launches `<command>` three times. It samples `VmRSS` and `RssAnon` from `/proc/<pid>/status` at 10 s and 30 s, records `Private_Dirty` through `scripts/proc-smaps-top`, sends TERM, and prints the median `VmRSS` per sample time. Run it once per variant and per binary (shell, desktop). Pass the real binary, not a wrapper that forks and exits.

Exit codes: 64 for usage, 1 for a missing file, a bad argument, or a process that exits before a sample. A run that exits early is not a result; fix the cause and rerun.

## 4. Recommendation

No recommendation until the measurements in section 3 exist. Two facts should drive it:

- Option A is what ships today; it is the only option measured in production.
- The `guard` calls claim isolation that release builds do not provide. Either make that true (Option B or C, after measurement) or stop claiming it in code comments (Option A).
