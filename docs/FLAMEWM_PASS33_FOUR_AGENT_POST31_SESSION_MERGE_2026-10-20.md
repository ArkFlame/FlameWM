# FlameWM Pass 33 — Four-agent + Pass 32 integrated source overlay

**State:** source merge complete; native Rust/real X11 session verification **not performed**. This source archive is **an overlay for the user's complete checkout**, not a self-contained binary release or rebuilt `.deb`.

## Inputs and provenance

| Input | SHA-256 | Changed relative to Pass 30 |
|---|---|---:|
| Pass 30 (`FlameWM_0.1.0_PASS30_RENAMED_HELPER_E0601_FIXED.tar.gz`) | `90029cd647bcd87972747cafff1843976522733e91621cb4fa7c25c9a083e989` | baseline |
| Pass 31 Agent 01 cursor placement | `17d49eca7f5ee05a431829fee4352550996284f85e6562298c3d6dbe15c8dbd7` | 3 |
| Pass 31 Agent 02 launch feedback | `ee6e2345c590a3f96ec0882f88ecae9e8921e88a711c17037a8a9fd394f129f6` | 14 |
| Pass 31 Agent 03 KWallet/Brave | `97c0b5d149722d15c57cc8704ca479c9c8c98fbfc462f560a6d7e3079b63d3bb` | 4 |
| Pass 31 Agent 04 display topology | `025b5fd80fb126ab931d457b6b4b67fe67283a23e22a6f3071c912ba21759d26` | 9 |
| Pass 32 desktop-services add-on | `368b52bdf429f65898977884a5e687dac47e4bd3cee271d7b6fc5baf3769ef84` | 5 new files + 5 guarded packaging modifications |

Source trees compared by content SHA-256 before applying. **Zero overlapping agent-modified paths**. Source merged in order Agent 01 → Agent 03 → Agent 02 → Agent 04; Pass 32 applied last. Original Pass 30 source preserved where no agent touched it. The stale `crates/flamewm-wm/src/bin/desktop_compat.rs` is removed, and `autobins = false` still guards Cargo target discovery.

### Changes integrated

1. **Initial window placement:** Agent 01 `frame/geometry.rs`, `size_hints.rs`, `wm/manage.rs` pointer-based placement for eligible initial maps.
2. **Launch feedback:** Agent 02 application startup-notification parser/broadcast, X11 observer, retained shell overlay, icon and optional animation.
3. **KWallet and Brave:** Agent 03 session-core/session-runner session compatibility and PAM wallet startup logic. **No profile or wallet migration or credential decryption has been verified on a live session.**
4. **Displays:** Agent 04 panel topology state, XRandR invalidation/synchronization and monitor-layout UI, validation and rollback code. Real mixed-resolution monitor behavior still requires hardware validation.
5. **Desktop services:** Pass 32 session wrapper, Polkit and notification-provider bootstrap, XDG autostart via dex, optional clipboard/Bluetooth helpers, read-only diagnostics and package dependencies. `packaging/flamewm-session.desktop` points to `/usr/libexec/flamewm/flamewm-session-wrapper`; `scripts/package` installs both helper scripts. `scripts/enable-post31-desktop-integration` is idempotent.

`./scripts/apply-pass33-merge` rechecks Cargo target migration, performs the guarded session integration and executes seven source/consumer contract checks. Run this script **after extracting** the overlay over the complete FlameWM source checkout and **before building**. Do not reapply older passes afterward.

## Verification performed (this sandbox)

- 81 **focused Python tests passed** across 12 test suites: Pass 23/24/26/27/28/29/30 regressions, package fixture, source-archive fixture, UI tests, 7 Pass 33 merge tests, and 22 desktop service tests.
- The previous Pass 27 two-monitor-only source tests were updated to assert the new multi-monitor planning/verification semantics. The Pass 24 panel test was tightened to check that outputs with no active mode cannot create panels. No behavioral gates removed.
- Package fixture updated to include Pass 32's **actual installed executables**, and continues to assert failure for unregistered executable scripts. **7/7 fixture tests passed** with the stubbed Cargo binary and `dpkg-deb` metadata checks.
- `bash -n` for changed session/package scripts passed; Python syntax checks passed; Pass 33 merge script succeeded repeatedly with `FLAMEWM-PASS33-MERGE-READY`.
- UI contract guard: **0 errors, 24 previously known warnings**.
- Desktop conversion source contract: passed.
- Cargo.lock manifests: all direct dependencies of the four relevant modified crates were present in the lockfile (not a compiler test).

### Not passed / blocked / unverified

- **No native Rust toolchain is installed in this sandbox.** Cargo compilation, Rust tests and Debian release build are **UNVERIFIED**. Do not treat this as an installable or release-certified `.deb`.
- The unscoped Python test discovery was stopped after reaching its runtime limit. The 81 focused tests above completed independently and are the available passing evidence, **not** a full-suite pass.
- The raw standalone Pass 30 overlay (and these Pass 31 derivatives) **does not include all files registered by the inherited source manifest** (notably several `assets/web/` and `.vendor/RustWebRender-0.0.9/` paths). As a result, a full real source-archive/package gate from the *isolated overlay* fails for absent baseline assets. Apply this over the user's **complete existing repository** and rerun the gate there; missing files cannot be invented here. This is separate from the passing tiny packaging fixture.
- Architecture guard: Pass 30 already reports 3 violations. The combined source introduces **3 additional reported violations**: Agent 02 adds `x11rb` directly in `flamewm-applications` (manifest and source), and Agent 03 adds another prefixed-environment assignment. These remain open architectural concerns; the guard was **not disabled or weakened**.
- Real KWallet unlocking, old Brave profile continuity, mixed-resolution XRandR position updates, taskbar relocation, third-party startup notification and Polkit/notification startup **require native user-session tests**.

## Apply overlay over complete FlameWM checkout

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_PASS33_ALL_FOUR_AGENTS_AND_SERVICES_MERGED.tar.gz"
test -f "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
    tar -xzf "$ARCHIVE" -C . && \
    ./scripts/apply-pass33-merge && \
    CARGO_BUILD_JOBS=2 ./scripts/check-cargo && \
    CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

After a real `FLAMEWM-DEB-READY`, install the exact produced file and launch a new session. Do not assume the original Pass 30 source overlay is a complete independently buildable checkout.

**Release decision:** SOURCE MERGED / FOCUSED TESTS GREEN / NATIVE RELEASE BLOCKED PENDING BUILD + HARDWARE + SECURITY ACCEPTANCE.
