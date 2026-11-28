# Unit p101-missing-deps-gaps

STATUS: OPEN

Scope: startup reads that depend on a missing system font, a missing session bus (D-Bus / portal), or missing XDG dirs / HOME / XDG_RUNTIME_DIR. Static read only. No crate edits, no git, no cargo, no python, no sleep. Compared against `docs/FAULT_TESTS.md`, which indexes five fault suites (accel, settings load, x11 startup, wayland backend, wrapper ldd) and none for fonts, D-Bus or HOME.

## Gap table

Outcome key: ABORT = startup stops; DEGRADE+N = continues with a notice (N = where the notice goes); SILENT = continues with no notice.

| # | file:line | Today | Missing fallback / defect | Test today |
| --- | --- | --- | --- | --- |
| G1 | `crates/flamewm-wm/src/bin/flamewm-session.rs:140` | HOME unset -> `PathBuf::from("/root")`; config home then derived from `/root` (XDG_CONFIG_HOME overrides only config). SILENT. | Wrong-user config root with no notice. Should be a typed notice and an XDG-only path plan, or an error. | none found in FAULT_TESTS.md |
| G2 | `crates/flamewm-wm-wayland/src/font.rs:45-50` (`search_roots`), `:75-95` (`load`) | Titlebar font: HOME unset skips `~/.local/share/fonts` silently; a candidate file that is unreadable (`:82`) or fails `Font::from_bytes` (`:87`) is skipped with no log. Only "no font at all" logs once (`:107-112`, DEGRADE to stderr, titlebar text skipped). | Per-candidate failure is SILENT; a bundled/unreadable font is indistinguishable from an absent one. Built-in glyph fallback exists only at caller level. | 0 `#[test]` in font.rs |
| G3 | `scripts/flamewm-wayland-session:467-480` | No `DBUS_SESSION_BUS_ADDRESS`, no `$XDG_RUNTIME_DIR/bus` socket, and no `dbus-run-session` on PATH: continues with `session_bus=absent`. Only a log line `dbus_session_bus=absent` (`:540`) and a cause suffix `+dbus_absent` (`:196`). No user notice. | Silent run with no bus; clients (portal, settings, lock D-Bus calls) fail later without a root cause. | `scripts/test-session-fallbacks` stubs `dbus-run-session` (`:118`) and sets a dead bus address (`:179`); the absent-bus-no-wrapper case is not listed in FAULT_TESTS.md |
| G4 | `scripts/flamewm-wayland-session:200-201, 241-245` | XDG_RUNTIME_DIR unset: stale-socket check is skipped (guard `-n`); `:470` falls back to `/run/user/$(id -u)`. SILENT. | Acceptable; the `/run/user/<uid>` fallback is itself unchecked. | covered: `scripts/test-session-fallbacks:193` (`unset XDG_RUNTIME_DIR`), `scripts/test-wrapper-faults:19` |
| G5 | `scripts/flamewm-wayland-session:101-102, 308-310` | HOME and XDG_CACHE_HOME both unset: chain cache and failure report are disabled (guarded). SILENT. | Fail-report disappears with no notice to the greeter/log. | not in FAULT_TESTS.md (test-session-fallbacks sets HOME in every case) |
| G6 | `crates/flamewm-render-x11/src/xft/backend.rs:43-62`, `crates/flamewm-render-x11/src/xft/fonts.rs:3-36` | libfontconfig missing: `eprintln!("FLAMEWM_RENDER_FONTCONFIG_WARNING ...")` (`:57-59`), continues with no fontconfig; then `font_for` fails and falls back to width estimate (`:138`). Bundled font path unreadable: `register_application_fonts` returns Err (`:16`), propagated by `?` (`:53`), so the whole Xft backend is dropped to width estimate. DEGRADE+stderr. | Degrade path is correct and noticed; not tested. Bundled fonts are silently not registered when fontconfig is absent. | none in FAULT_TESTS.md |
| G7 | `crates/flamewm-lock/src/session.rs:58-62, 111-115` | No session bus: `ControlClient::connect` fails, eprintln, uses defaults or full-root outputs. DEGRADE+stderr. | Correct degrade; the notice is not visible to a user on the lock screen. Not tested. | none |
| G8 | `crates/flamewm-lock/src/pam.rs:144-150` | libpam not loadable: `record_failure("pam", ...)` and `Outcome::Unavailable`. DEGRADE+failure record. | Correct and recorded. The effect of `Unavailable` on the unlock path was not traced (UNVERIFIED). | none in FAULT_TESTS.md |
| G9 | `crates/flamewm-api/src/paths.rs:8-10, 14-` | `home_dir()` returns None when HOME is unset; `xdg_dir` returns None when neither var resolves. Callers not audited in this unit. | UNVERIFIED: each caller's None branch needs the same check as G1. | none |
| G10 | `crates/flamewm-session-core/src/diagnostics.rs:258-265` | `runtime_dir_finding`: XDG_RUNTIME_DIR unset -> Severity::Degraded finding ("D-Bus session services ... may fail"). Diagnostic path only. | Good notice, but it is a doctor report, not a startup notice. | diagnostics tests not checked in this unit |

## Fix specs (at most 3)

**Fix 1 (G1, highest risk).** File: `crates/flamewm-wm/src/bin/flamewm-session.rs:140`.
- Replace `unwrap to /root` with: if HOME is unset or empty, emit `flamewm-session: HOME unset; config under XDG_CONFIG_HOME only; blast_radius=degraded:settings-paths` to stderr, and build `paths` from `XDG_CONFIG_HOME`/`XDG_DATA_HOME` with no home-relative default. If neither XDG var is set either, return `Err("HOME and XDG_CONFIG_HOME unset")` at the same place the `Result` already flows.
- Keep `default_paths_from_home` unchanged for the set-HOME case.
- Test: add `#[test]` in `flamewm-wm` binary tests with `env` injected via a `fn paths_from(home: Option<OsString>, xdg: Option<OsString>)` seam. Assert: HOME unset + XDG_CONFIG_HOME set yields that config root; both unset yields Err; neither case produces a path starting with `/root`.
- Gate: `cargo test -p flamewm-wm -- paths_from` (orchestrator runs it).

**Fix 2 (G2).** File: `crates/flamewm-wm-wayland/src/font.rs:75-95`.
- Add `fn load_from(roots: &[PathBuf]) -> Option<TitleFont>` and have `load()` call it with `search_roots()`. Inside the candidate loop, record the first read error and first parse error per candidate in a `Vec<String>`; when `load_from` returns None, the existing `NO_FONT_LOG` message appends the list, e.g. `candidate=DejaVuSans.ttf reason=parse`.
- Keep the cache and `FontSlot` unchanged.
- Test: `#[test]` with a `tempfile`-free temp dir under `std::env::temp_dir()` plus a unique name: (a) empty root gives None and the error list is empty; (b) a root with a 0-byte `DejaVuSans.ttf` gives None and the list names `reason=parse`; (c) a root with an unreadable file (chmod 000, skip if running as root) names `reason=read`.
- Gate: `cargo test -p flamewm-wm-wayland -- font::`.

**Fix 3 (G3).** File: `scripts/flamewm-wayland-session:467-480`.
- In the `absent` branch where `dbus-run-session` is missing (after the `command -v` check fails), write a notice to the session log and the failure record: `dbus=absent wrapper=missing blast_radius=degraded:portal,settings,lock-d-bus`, then continue (do not abort; keep current behavior).
- Add a case to `scripts/test-session-fallbacks`: unset `DBUS_SESSION_BUS_ADDRESS`, unset `XDG_RUNTIME_DIR` socket, remove `dbus-run-session` from the stub PATH, and assert the log contains `dbus=absent wrapper=missing` and the session still reaches the backend stage.
- Add the case as row 6 to `docs/FAULT_TESTS.md` inventory and the `CASES` table of `scripts/run-fault-tests`.
- Verification: `bash scripts/test-session-fallbacks` (orchestrator runs it; this unit did not).

## Evidence and limits

- Static read only. `rg`, `sed`, `grep` and `Read` were used. No build, no test, no run.
- UNVERIFIED: G8 `Outcome::Unavailable` effect on unlock; G9 caller behavior; G2 real behavior under a missing font (no run).
- Existing coverage (FAULT_TESTS.md) does not include fonts, D-Bus absence or HOME-unset startup. The FAULT_TESTS.md index is authoritative only for the five listed suites.
