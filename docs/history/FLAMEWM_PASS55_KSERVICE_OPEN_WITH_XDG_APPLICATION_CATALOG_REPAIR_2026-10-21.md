# FlameWM Pass 55 — Dolphin/KService Open With integration

**Basis:** User-supplied `005_21_10_26.tar.gz` (current FlameWM 0.1.1 source overlay), `associations-sources-reference.tar.gz` (upstream KDE KService, LXQt config/libqtxdg, Thunar and portals), previous MIME diagnosis. This pass is a **source-integrated repair**, not a binary or a certified live-X11 release.

## Deterministic source defect

The native session already mirrored MIME defaults and `Added Associations` in `crates/flamewm-wm/src/session_app_compat.rs`. But it never established `XDG_MENU_PREFIX`, never provided `${XDG_MENU_PREFIX}applications.menu` in the active XDG configuration search path, and omitted the menu prefix from D-Bus/systemd activation synchronization. Thus Dolphin/KService could find MIME defaults yet build an empty or incompatible KSycoca application catalog. `xdg-desktop-portal` is not the application's primary native chooser and is deliberately left unchanged.

The freedesktop Desktop Menu Specification requires `${XDG_CONFIG_DIRS}/menus/${XDG_MENU_PREFIX}applications.menu`; KService's bundled `src/fallback-applications.menu` contains `DefaultAppDirs` and its layout. FlameWM now ships an analogous *FlameWM-owned* standards-compliant menu and explicitly includes `<Include><All/></Include>` so installed applications under XDG_DATA_HOME and XDG_DATA_DIRS can populate the catalog. The menu also supports default merged application menus and directory entries.

## Implemented

- `packaging/menus/flamewm-applications.menu`: valid XML menu; scans all standard XDG application desktop entries and the merged application menus. No hard-coded app list or MIME database.
- `flamewm-session-core`: unconditionally sets `XDG_MENU_PREFIX=flamewm-` in every native process plan, preserving `XDG_CURRENT_DESKTOP=FlameWM` even when KDE identity was inherited.
- `flamewm-wm/session_app_compat.rs`: installs menu atomically under the already-owned XDG_CONFIG_DIRS overlay. User `~/.config/menus/flamewm-applications.menu` remains higher priority. Creates optional data directory even when Brave overlays are absent so the autostart companion consistently discovers both overlays. Includes a Rust menu unit test.
- `flamewm-session`: synchronizes `XDG_MENU_PREFIX` to D-Bus/systemd user activation, with a Rust regression assertion. This matters for KIO processes started after login.
- `flamewm-session-wrapper`: enforces FlameWM desktop identity and menu prefix for sibling autostart services, so a stale SDDM/KDE environment cannot override them.
- `flamewm_desktop_services.py`: passes the prefix on to autostarted apps; the compat data directory is now created by the session owner before they start.
- `scripts/xephyr`: direct WM test sessions add the checked-in menu's directory to `XDG_CONFIG_DIRS` and export `flamewm-`. This avoids false negatives when the normal session supervisor is intentionally bypassed.
- `scripts/package`, `packaging/install-manifest.txt`: install the XML menu to `/etc/xdg/menus/flamewm-applications.menu` as a Debian conffile; also include it in the build's source prerequisites. Installed menus and the session-local owned overlay agree.
- `tools/verify_flamewm_open_with.py`: a safe real-session diagnostic. With `--refresh`, invoke installed KDE `kbuildsycoca6`/`kbuildsycoca5 --noincremental` to rebuild the user's own KDE application catalog; without it, read-only verification. Does not alter `mimeapps.list`.
- `tools/test_flamewm_pass55_open_with_contract.py`: menu XML, first-file XDG precedence, session/D-Bus/autostart paths, and read-only diagnostic contracts.
- `tools/test_flamewm_package_pipeline.py`: update packaging fixture to include new menu and remove stale `0.1.0` hardcoding; actual source is version `0.1.1`. Verify the `.deb` file list includes the new menu.

## Scope and safety

No new daemon; no KDE/Plasma masquerade; no `/etc/xdg` modifications at runtime; no blanket writes to user MIME defaults; no purging KDE caches; no native KService reimplementation. GTK/GIO/Flatpak MIME-default stores remain authoritative. Applications already present in the session retain their old inherited environment until relaunch. KService's application cache requires a *new session* and possibly `kbuildsycoca6 --noincremental` after installing an updated package.

## Evidence and limitations

- Python/packaging tests pass in this execution container. `test_flamewm_package_pipeline.py` uses fixture binaries, not compiled FlameWM programs; its Debian assertions verify installation paths and package checksums.
- The uploaded overlay lacks the user's complete runtime assets, `Cargo`/`rustc` and KDE's `kbuildsycoca6` are absent in this container. **No Rust compilation, real Dolphin launch, or live KSycoca MIME service query was executed.** Feature is source integrated but runtime unverified; do not claim four-issue or OS-wide certification.
- One unrelated pre-existing legacy source-guard (`test_flamewm_pass50_integration_contract.py`) checks `(*new_set).images.is_null()`; the newer source no longer constructs `new_set` for cursor shadows. It is not included in the MIME verification gate.

## Native verification on the user's complete Linux checkout

Apply the latest Pass 55 overlay only; never replay Pass 41–54 over it. Then:

```bash
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
python3 -m unittest -q tools.test_flamewm_pass55_open_with_contract tools.test_flamewm_package_pipeline
CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-session-core -p flamewm-wm
CARGO_BUILD_JOBS=2 ./scripts/build-deb
sudo apt install "$(pwd)/.target/dist/flamewm_0.1.1_amd64.deb"
```

Log out and sign back into the **FlameWM** session (to replace the session process and D-Bus activation environment). Close pre-existing Dolphin processes, then from a FlameWM terminal:

```bash
printf 'desktop=%s menu=%s\n' "$XDG_CURRENT_DESKTOP" "$XDG_MENU_PREFIX"
python3 tools/verify_flamewm_open_with.py --refresh
xdg-mime query default text/plain
xdg-mime query default application/pdf
# Launch a new Dolphin, then right-click a PDF, text document, PNG and video and inspect Open With.
dolphin
```

**PASS:** `XDG_CURRENT_DESKTOP=FlameWM`, `XDG_MENU_PREFIX=flamewm-`; diagnostics resolves an existing `flamewm-applications.menu`, detects installed `.desktop` entries and rebuilds KSycoca successfully; Dolphin shows installed compatible apps, choosing one persists in the standard user `mimeapps.list`, and chooser remains populated after relogin. Test Flatpak handlers where installed, including apps from `XDG_DATA_DIRS`.

**FAIL → evidence:** save full `kbuildsycoca6 --noincremental` output, `env | grep '^XDG_'`, `ls -l /etc/xdg/menus/*applications.menu`, `xdg-mime query default text/plain`, `gio mime text/plain`, and affected `.desktop` file (`MimeType=`, `TryExec=`, `OnlyShowIn=`). Do **not** reset `~/.config/mimeapps.list` or remove `.cache` without a discriminating runtime observation. If KSycoca menu is healthy but specific apps are hidden, trace their `.desktop` `MimeType` and desktop visibility fields separately rather than altering the session menu blindly.

## External reference behavior

- Freedesktop Desktop Menu specification: https://specifications.freedesktop.org/menu/latest-single/
- KDE KService `src/fallback-applications.menu`, `src/services/kapplicationtrader.cpp`: extracted from user-supplied KService GitHub source zip.
- KDE discussion of `XDG_MENU_PREFIX` and `kbuildsycoca6`: https://discuss.kde.org/t/kbuildsycoca6-applications-menu-not-found-in-home-user-config-menus-etc-xdg-menus/17312
- LXQt association editing: `lxqt-config-file-associations/` inside the supplied LXQt source. No code from LXQt, KDE or other projects was copied into production source.
