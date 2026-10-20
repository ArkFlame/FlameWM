# FlameWM Pass 31 / Agent 03 — Execution handoff and integration report

**Status:** Source patch delivered; live SDDM/Brave/KWallet verification **UNVERIFIED**. This report must not be interpreted as successful restoration of the user's real browser sessions.

## Baseline and scope

- Input: `FlameWM_0.1.0_PASS30_RENAMED_HELPER_E0601_FIXED.tar(3).gz`.
- Verified baseline SHA-256: `90029cd647bcd87972747cafff1843976522733e91621cb4fa7c25c9a083e989`.
- Input is a **source overlay** (912 archive entries), not the complete installed project checkout. No `.git` directory was supplied; a real branch/commit and current user working-tree state cannot be verified here.
- Source identity: FlameWM independent Rust X11 desktop, **not** the historical IceWM C++ fork. The original Pass 31 Agent 03 handoff and current Rust source are the governing sources. IceWM/C++ reports were treated as historical reference only.
- Exclusive production edits: `crates/flamewm-session-core/src/lib.rs`, `crates/flamewm-wm/src/session_app_compat.rs`, `crates/flamewm-wm/src/bin/flamewm-session.rs`. `crates/flamewm-applications/src/lib.rs` was inspected but **not modified** (Agent 02 owns it). No display/XRandR code, user profile, wallet, KDE settings, `packaging/debian/control.in` or PAM configuration was modified.

## Source-evidenced failure mechanisms

1. Pass 30 selected `KDE_SESSION_VERSION=6` whenever a KWallet6 executable/service file was installed, even when both KWallet5/6 exist or when the user may have used another Chromium encryption backend. Installation is **not proof** of original OS Crypt key provenance. Chromium selects different key stores for KDE5/KDE6; wrong selection can make existing encrypted browser data unavailable.
2. The generated Brave `.desktop` wrapper previously set `XDG_CURRENT_DESKTOP=KDE` **without pinning the corresponding KDE session generation in the same Exec**. Direct FlameWM launch did the same. Chromium's KDE generation detection can then select the wrong OS Crypt backend. Separately, installed `.desktop` actions with explicit `--password-store` must not be altered.
3. Pass 30 ran `pam_kwallet_init` based only on a guessed wallet daemon being installed and detached its completion asynchronously. KDE's helper is a continuation of a *pre-existing PAM socket handshake*, not a password-retrieval/unlock substitute. Starting it without a PAM socket cannot synthesize an SDDM login password.
4. A stale generated Brave desktop entry can survive after backend selection changes because the previous code only generated new entries; it did not retire its own previous overlay when generation was no longer known.

**Actual root cause on the user's physical Plasma→FlameWM migration is NOT classified** without nonsecret comparative host/session evidence. These are verified source hazards, not proof of a specific wallet key or of recovered cookies.

## Implemented

- Pure `resolve_kwallet_version` with explicit `FLAMEWM_BRAVE_PASSWORD_STORE=kwallet5|kwallet6` hint, inherited KDE session provenance only when paired with an actual inherited KDE desktop identity, and a conservative single-installed-generation fallback. Both-generation/no-provenance stays ambiguous (`None`). Explicit non-KDE selection does not generate a KDE-specific Brave `.desktop` wrapper. This is a **generation hint**, not a claim of original encryption provenance, not a password-store override, and not an instruction to use plaintext.
- Keep global `XDG_CURRENT_DESKTOP=FlameWM` and `XDG_SESSION_DESKTOP=flamewm`; never export global `KDE`. Browser-only `.desktop` wrappers use matching `XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=5|6`. Existing explicit `--password-store` (both `=value` and separate forms), explicit desktop environment, all `--user-data-dir` / `--profile-directory` parameters, desktop actions, URIs/field codes, and custom launch flags stay intact. An installed/user launcher is never rewritten.
- Identify FlameWM-owned generated Brave overlays and remove only these stale copies when the wallet generation is no longer selected; preserve unmarked files and symlink targets. Preserve the existing per-session MIME fallback and XDG directory precedence.
- `pam_kwallet_init` is attempted only if `PAM_KWALLET5_LOGIN` exists and refers to a real Unix socket. Execute once, wait for completion (bounded timeout), reap/kill on failure, and emit only nonsecret diagnostics. When the socket/module/helper is missing, do not pretend auto-unlock succeeded. Never prompt for/store/export the user's password.
- D-Bus/systemd activation receives the FlameWM desktop identity and an explicitly empty `KDE_SESSION_VERSION=` if none was established, preventing retention of a stale Plasma backend hint. PAM-specific variables are never published as activation-environment keys.
- Rust unit regressions added for wallet5-only/wallet6-only/both/neither, explicit selection and stale inherited version, Brave action flag/profile preservation, stale owned overlay cleanup, missing/valid synthetic PAM socket, and stale D-Bus activation environment.

## Mandatory Agent 02 reconciliation

`INTEGRATION_REQUIRED: crates/flamewm-applications/src/lib.rs; function launch_argv;` the Pass 30 direct launcher still sets `XDG_CURRENT_DESKTOP=KDE` for Brave unconditionally whenever the session is FlameWM, including when `KDE_SESSION_VERSION` is absent/ambiguous or explicit `FLAMEWM_BRAVE_PASSWORD_STORE=gnome-libsecret|none` contradicts it. Agent 02 must use the same conditional backend policy: honor explicit browser command flags (`--password-store` including space-separated syntax), user/profile flags, and desktop identity; set a KDE identity **only when** a selected compatible KDE generation is known; never set `basic` or change user profile; add focused direct-launch tests. Session-generated `.desktop` wrappers and direct shell-app launches must agree. Agent 03 did not edit the Agent 02-owned file.

**Uncontrolled manual terminal launches:** a normal terminal session intentionally has `XDG_CURRENT_DESKTOP=FlameWM`. FlameWM cannot rewrite arbitrary shell commands to enforce a browser backend; manual launch must be compared with the user's existing original flags/environment. Never set global `KDE` to conceal this distinction.

## Verification performed

- Source archive SHA-256: **PASS**, exact handoff baseline match.
- `python -m unittest discover -s tools -p 'test_flamewm_pass2*.py' -q`: **PASS**, 37 tests.
- `python -m unittest tools.test_flamewm_pass30_helper_migration -q`: **PASS**, 3 tests.
- `python -m unittest tools.test_flamewm_pass27_integration tools.test_flamewm_pass30_helper_migration tools.test_flamewm_pass29_cargo_targets -v`: **PASS**, 13 tests (overlapping with preceding suites).
- Rust unit regression tests: **ADDED, NOT EXECUTED**. This authoring container has no `rustc`, `cargo`, or `rustfmt`.
- `cargo fmt --all -- --check`, `cargo test -p flamewm-session-core`, `cargo test -p flamewm-wm`, `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`, `./scripts/build-deb`: **UNVERIFIED**, toolchain and complete workspace assets unavailable here. Do not report PASS.
- Live SDDM PAM chain, KWallet5/6 daemon state, same-profile Brave cookie/password decryption, KScreen absence, logout/login continuity: **USER-OWNED / UNVERIFIED**. No secrets, profiles, or wallet key material were accessed.
- Provided `MANIFEST.sha256` is historical and includes omitted paths; it was deliberately not misrepresented or regenerated as a full archive manifest. The generated `.tar.gz` is independently checked with `tar -tzf` and SHA-256.

## Acceptance grades

| Contract | Status | Reason |
|---|---|---|
| A1 — same Brave profile/encryption backend | **PARTIAL / USER-OWNED** | Preserve original argv and bind per-app KDE generation; no host key provenance or live decrypted-data proof; Agent 02 direct launcher still needs reconciliation |
| A2 — KWallet SDDM/PAM auto-unlock | **PARTIAL / USER-OWNED** | Correct handshake precondition and ordering implemented; host login PAM stack/password compatibility unknown |
| A3 — no desktop side effects | **SOURCE-VERIFIED / RUNTIME UNVERIFIED** | Global FlameWM identity and existing display code remain unchanged; requires live KScreen/XRandR smoke test |
| A4 — safe startup | **SOURCE-VERIFIED / RUNTIME UNVERIFIED** | No new daemon/secret migration/plaintext fallback, owned overlay cleanup, bounded once-only helper; requires native test/host proof |

## Build and physical acceptance on the actual checkout

Apply the source overlay onto the **complete existing Pass 30 checkout**, without replaying Pass 27–29. Preserve any dirty work and review the patch before application. Then run:

```bash
cargo fmt --all -- --check
cargo test -p flamewm-session-core
cargo test -p flamewm-wm
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
./scripts/build-deb
```

If formatting differs, run `cargo fmt --all` only after reviewing the exact changes. Never modify tests merely to make the commands green. Check the actual SDDM PAM service + includes, environment metadata, selected Brave profile **path only**, original runtime arguments with sensitive values redacted, KWallet service/bus status, and profile login continuity (only `persisted/not persisted`). If any previously encrypted data cannot be read, **stop**: never delete/copy/rekey browser profiles, replace wallets, or switch to `--password-store=basic`.

## Primary external evidence

- Chromium Linux Password Storage: https://chromium.googlesource.com/chromium/src/+/main/docs/linux/password_storage.md
- Chromium selected Linux backends: https://chromium.googlesource.com/chromium/src/+/refs/tags/124.0.6351.1/components/os_crypt/sync/key_storage_util_linux.cc
- KDE kwallet-pam handoff mechanism: https://github.com/KDE/kwallet-pam
- Upstream `pam_kwallet_init` helper uses the `PAM_KWALLET5_LOGIN` socket: https://github.com/KDE/kwallet-pam/blob/master/pam_kwallet_init
