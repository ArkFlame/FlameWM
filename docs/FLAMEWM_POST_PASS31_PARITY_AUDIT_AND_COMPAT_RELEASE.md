# FlameWM 0.1.0 — Post-Pass-31 desktop-parity simulation and independent integration addon

**Baseline SHA-256:** `90029cd647bcd87972747cafff1843976522733e91621cb4fa7c25c9a083e989` (`FlameWM_0.1.0_PASS30_RENAMED_HELPER_E0601_FIXED.tar.gz`).

**Scope and proof:** This audit is grounded in the actual Pass 30 source overlay and the four Pass 31 handoffs. The Pass 31 patches **were not supplied and have not been merged or run**. The comparison below assumes their explicit acceptance contracts eventually pass. A handoff is not proof of completed behavior.

**Project identity:** independent Rust/X11 desktop. IceWM/C++ documents in the project are historical reference only. The supplied `plasma-desktop-master.zip` (5,301 entries) contains desktop shell and `screenmapper.cpp`, but not KWin, KScreen, KWallet/PAM or PowerDevil; it cannot verify those runtimes. `xfwm4-master.zip` (1,256 entries) supplies XFWM window/session/startup-notification code, not the whole XFCE desktop. `lxqt-master.zip` (52 entries) is predominantly component/submodule references, not fully materialized notification/Polkit/power implementation source. This reference-set boundary prevents false feature-parity claims.

## 1. Current verified features and intended Pass 31 changes

| Area | Pass 30 source evidence | Pass 31 simulated state | Still required |
|---|---|---|---|
| EWMH window management, workspaces, Meta interactions | `crates/flamewm-wm-x11`, `flamewm-window-core` | Agent 01 pointer-aware initial-map placement | Real ICCCM/EWMH and mixed-monitor runtime tests |
| Start, taskbar, status popovers, system tray, clock | `flamewm-shell`, `flamewm-shell-core` | Agent 04 panel re-anchor after displays move | Real-monitor visual and input validation |
| Display Settings, RandR | `flamewm-settings` / `flamewm-platform` / `flamewm-wm-x11` | Agent 04 spatial UI and transactional output changes | Hardware/output-mode generation and rollback proof |
| Window-launch feedback | `flamewm-applications` | Agent 02 cursor badge with startup completion | Native startup association and timeout proof |
| KWallet and Brave | `flamewm-session-core`, `flamewm-wm/src/session_app_compat.rs` | Agent 03 root-cause repair | SDDM/PAM and existing user-profile end-to-end proof; not covered by this addon |
| Screen locking/screensaver | `flamewm-lock`, `wm-x11/src/idle.rs`, `flamewm-ribbons` | Unchanged | Native secure-input, suspend/resume and multi-monitor verification |
| Audio/Wi-Fi/MPRIS | `flamewm-integrations-linux/{pulse,network_manager,mpris}` | Unchanged | Device/hotplug and session lifecycle tests |
| MIME/portal integration | `session_app_compat.rs`, `packaging/xdg-desktop-portal/flamewm-portals.conf` | Agent 03 preserves settings | FileChooser/OpenURI/Flatpak application-specific probes |
| Clipboard/X selection | `flamewm-x11-canary/src/scenarios/clipboard.rs` | Unchanged | Clipboard history/persistence requires external optional manager |

## 2. Post-Pass-31 feature-gap matrix vs Plasma/XFCE/LXQt

Status means *source-audited*, not product-runtime-certified. "External" means FlameWM should integrate existing standard providers instead of duplicating their engines.

| Domain | Gap/condition after Pass 31 | Recommendation | This addon |
|---|---|---|---|
| Notification D-Bus | No complete notification server is bundled with FlameWM | Prefer D-Bus-activatable installed `xfce4-notifyd`; fallback `dunst` or LXQt | Detect existing server and avoid competing instance; fallback launch when no D-Bus provider exists |
| Polkit authentication | FlameWM has no registered session authentication agent | Reuse `lxqt-policykit-agent` or installed KDE/GNOME agent | Start one when no known agent is active; stop at logout |
| Freedesktop user autostart | Session supervisor explicitly launches exactly WM/Desktop/Shell | Delegate `.desktop` filtering, hidden entries and `OnlyShowIn` to `dex` | Add one-time `dex --autostart --environment FlameWM` gated by WM readiness |
| Clipboard history | No full history manager proved | Optional `xfce4-clipman`/`qlipper`/`copyq`; never record clipboard by default | Opt-in via `FLAMEWM_ENABLE_CLIPBOARD_HISTORY=1` |
| Bluetooth UI | NetworkManager provides Wi-Fi, not a complete Bluetooth desktop stack | Use BlueZ and opt-in `blueman-applet` until native integration | Opt-in via `FLAMEWM_ENABLE_BLUETOOTH_APPLET=1` |
| Removable media & trash | Desktop has Trash; host UDisks2/GLib/GVfs not owned by FlameWM | Package UDisks2/GVfs support where Dolphin/thunar needs it | Doctor reports tool availability; no competing device daemon |
| Laptop power management | Battery popover/idle lock exist, policy-daemon functionality not established | Integrate logind/UPower and inhibitors, suspend/resume; do not autostart a competing power manager | **Not implemented**: risky conflict with native lock/DPMS |
| Flatpak portal coverage | Desktop-specific portal config exists, default GTK with KDE FileChooser/OpenURI | Test ScreenCast/RemoteDesktop, OpenURI/FileChooser and document unsupported backends | Readiness check only; **no ScreenCast claim** |
| Notifications history/do-not-disturb | No authoritative notification center proved | Can be native separate feature, needs retained UI/shell work | **Not implemented**, reserved shell files |
| Global search/runner/index | Start search exists; semantic file-content search not proved | Optional external tracker/baloo if requested and measured | **Not implemented** (indexers can be expensive) |
| File thumbnails/previews | File manager app owns previews; MIME/thumbnail provider installation dependent | Use KDE thumbnailers/Tumbler/GVfs as optional services | **Not implemented** |
| Authentication/secret storage | KWallet/Brave continuity assigned to Agent 03 | Require native SDDM/PAM/system bus comparison | **Not modified**, avoids conflict |
| Screenshots/recording/share | X11 screenshot utilities can be installed; full portal ScreenCast not guaranteed | Separate portal/compositor compatibility work | **Not implemented** |
| Accessibility/i18n/input methods | No full AT-SPI/IME/session setup proved | Host `at-spi2-core`, IBus/Fcitx5 where installed | **Not implemented**; XDG autostart may pick appropriately installed services |
| Color management/scaling | Per-monitor scale partially implemented | ICC, gamma, color/night-light should be separate typed owners | **Not implemented**, Agent 04 owns outputs |
| Wayland | Rust/X11 WM is not a Wayland compositor | Modular Wayland provider as separate milestone | **Not implemented** |

## 3. Implemented here: session compatibility layer (additive, independent)

- `tools/flamewm_desktop_services.py` — single companion launched in FlameWM sessions only. A private runtime lock prevents duplicates. It waits for `_NET_SUPPORTING_WM_CHECK` (one-time startup poll only) and never launches KDE-only display manager services.
- After WM readiness it derives the same session-specific `flamewm/session-compat/{config,data}` XDG overlays generated by Pass 30 (when those directories exist). This prevents `dex` children from missing FlameWM MIME/Brave launcher overrides even though the wrapper process is a sibling of the Rust session supervisor. The implementation never exports `XDG_CURRENT_DESKTOP=KDE` globally. If Agent 03 changes this ownership/path, the post-merge integrator must reconcile that interface before shipping.
- XDG autostart is delegated to `dex` rather than a custom `.desktop` parser. `OnlyShowIn`, `NotShowIn`, `Hidden`, `TryExec` and user override semantics are thus the provider's responsibility. A missing `dex` degrades gracefully.
- PolicyKit chooses installed LXQt/KDE/GNOME agent only when an existing known policy agent is not detected. XDG autostart executes first; provider presence is rechecked afterward to reduce duplicate agents. Does not touch policy rules or elevate permissions.
- Notification provider respects D-Bus activatable service and current D-Bus owner, including activation files under user `XDG_DATA_HOME` and `XDG_DATA_DIRS`. It starts a fallback daemon only when one is not available. Does not own `org.freedesktop.Notifications` itself.
- Optional applets disabled by default: clipboard history (privacy) and Bluetooth tray (avoid duplicate tray), controlled via environment variables.
- `scripts/flamewm-session-wrapper` supervises companion lifetime with `flamewm-session`; preserves FlameWM session exit code and forwards stop signals.
- `scripts/enable-post31-desktop-integration` is **explicitly post-merge**. It edits only 5 package/build manifest files, preserves Agent 03's package recommendations, verifies baseline anchors first, and is idempotent. If Agent 03 changed session Exec or packaging layout incompatibly, it refuses modification. That is an integration conflict, not permission to overwrite Agent 03.
- `--doctor` emits non-secret JSON. It never opens KWallet, Brave profiles, cookies or password stores.

### Session/runtime dependencies

Recommends added by post-merge hook: `dex`, `lxqt-policykit`, `xfce4-notifyd`, `python3`, `x11-utils`. The first three are established platform providers. None are added to the Rust dependency graph. `xprop` comes from `x11-utils` and is used only during startup synchronization. Python 3 stays resident while the session companion is active; memory/PSS has **not** been measured.

This does *not* become a notification center, Polkit rule engine or new session authority. Existing services continue to own their contracts.

## 4. Merge after the four other agents

In the complete user repository, merge and verify the four agent branches first in their prescribed order:

`Agent 01 -> Agent 03 -> Agent 02 -> Agent 04`, one build owner, independent verifier, no blind tar extraction over dirty work.

Then from the repository root, apply this add-on's archive overlay (only **new files** under `scripts/`, `tools/` and `docs/`) and execute:

```bash
./scripts/enable-post31-desktop-integration --dry-run
./scripts/enable-post31-desktop-integration
python3 -m unittest discover -s tools/tests -p test_flamewm_desktop_services.py -v
bash -n scripts/package scripts/flamewm-session-wrapper
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Never run the hook against an unknown repo version. If it reports `FLAMEWM-INTEGRATION-BLOCKED`, resolve the reported precise owner conflict; it will not silently patch an unrecognized login setup. The wrapper is installed at `/usr/libexec/flamewm/flamewm-session-wrapper` by the updated packaging script. It is selected by the modified packaged `/usr/share/xsessions/flamewm.desktop` **only after the rebuilt `.deb` is installed and a new session begins**.

Read-only diagnostics in a FlameWM session:

```bash
/usr/libexec/flamewm/flamewm-desktop-services --doctor
/usr/libexec/flamewm/flamewm-desktop-services --plan
```

The old KDE/Plasma session remains unchanged. `XDG_CURRENT_DESKTOP` remains `FlameWM` inside a FlameWM session; this add-on never impersonates KDE to launch wallet or portal services.

## 5. Red/green gates / acceptance

- A1: `dex` runs only for FlameWM and only after the WM announces readiness; never launches Plasma-only `OnlyShowIn=KDE` services, never uses shell parsing for `.desktop` Exec.
- A2: Never start duplicate notification server when one is owned/activatable; fallback only if available.
- A3: Start at most one known policy agent instance, with no privilege escalation by the companion itself.
- A4: No clipboard history or Bluetooth tray without explicit opt-in; preserved FlameWM desktop identity; no changes to wallets/cookies/monitor layout.
- A5: Session exit tears down only child processes owned by the companion; `flamewm-session` exit status remains authoritative.
- A6: Packaging hook is idempotent, edits only verified five files, preserves all other agents' changes or blocks; package manifest includes installed helper/wrapper and executable source registration.
- A7: Cargo checks, .deb creation, normal KDE-vs-FlameWM startup, native D-Bus policy/notification interactions, and PSS measurements require the actual machine and remain **UNVERIFIED** here.

## 6. Evidence and remaining shipping blockers

**Executed:** 22 Python focused regression tests (including user D-Bus service discovery, deferred XDG overlay propagation, and rechecking for agents after dex), shell static syntax checks, post-merge hook dry-run/apply/reapply test on Pass 30 packaging copies, source-file ownership audit, baseline archive SHA-256. The four Pass 31 patches are not supplied; no native Rust compiler is installed here. No graphical X server/real PAM secrets available.

**Shipping blockers beyond the four agent branches:** actual D-Bus notifications and auth dialogs; real XDG autostart precedence; browser KWallet decryption; RandR panel relocation; hotplug, suspend/resume and X11 screen lock; ScreenCast portal compatibility (if claimed); mixed-DPI/3-4 monitors; actual resident PSS/idle CPU measurement; accessibility/input method testing; package install/restart/upgrade/uninstall testing. None of these can be marked PASS using source-only tests.

## 7. Relevant standards / external implementation references

- [XDG Desktop Entry Specification](https://xdg.pages.freedesktop.org/xdg-specs/desktop-entry/latest-single/)
- [XDG Autostart Specification](https://specifications.freedesktop.org/autostart-spec/latest/)
- [XDG Desktop Notifications Specification](https://specifications.freedesktop.org/notification/latest-single/)
- [XDG portal configuration](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html)
- [`dex` autostart provider](https://manpages.ubuntu.com/manpages/noble/man1/dex.1.html)
- [LXQt PolicyKit agent](https://github.com/lxqt/lxqt-policykit)
- [LXQt component boundaries](https://github.com/lxqt/lxqt/wiki/How-To-Release-A-New-Version-of-LXQt)

**Release verdict:** Companion implementation source and packaging integration contract are ready to merge. **This is not a claim that FlameWM has reached full Plasma/XFCE/LXQt feature parity.**
