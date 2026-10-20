# FlameWM 0.1.0 — Pass 29, incremental correction over Pass 27 / Pass 28

## Source authority

Based on the complete Pass 28 workspace, which includes the prior repairs to (1) Dolphin MIME/application opening and desktop identity, (2) XRandR panel/Start positioning, and (3) Brave/KWallet compatibility. This is a **source overlay** on the user's complete checkout; original packaged assets are not bundled here. It includes the Pass 28 changes: do not extract Pass 27 again after applying it.

## Reported failure E0601: `desktop_compat` missing `main`

**Proven root cause:** `crates/flamewm-wm/src/bin/desktop_compat.rs` was intended as a private module (`mod desktop_compat;` in `flamewm-session.rs`). Cargo 2024 edition auto-discovers direct `src/bin/*.rs` files as independent executable targets even when another `[[bin]]` is explicitly declared. When `cargo check --workspace --all-targets` traverses all binary targets, rustc compiles the helper as a standalone executable and reports E0601 because it has no `fn main`.

**Fix:** Declare `autobins = false` in the `flamewm-wm` package, and explicitly list both supported binaries in `crates/flamewm-wm/Cargo.toml`: `flamewm` and `flamewm-session`. The shared helper remains a compiled module of `flamewm-session`, but no longer becomes its own executable. This is Cargo's documented fix for this exact target-discovery situation. No dummy main, skipped tests, or disabled workspace checks.

## MIME fallback improvement

The Pass 28 compatibility helper already mirrors KDE defaults into a session-scoped `flamewm-mimeapps.list` under an XDG_CONFIG_DIRS prefix, without editing the user's KDE MIME settings, and already falls back to installed Kate/media players if no KDE entry exists.

**Additional proven source gap:** It treated a *present but stale* KDE default as resolved: e.g. `video/mp4=org.kde.haruna.desktop;` when Haruna is no longer installed, even if VLC is installed. Pass 29 validates installed candidates for `text/plain` and `video/mp4` and preserves KDE preference order among installed candidates; if none remains, it selects the first available configured fallback. If no application is installed, it leaves resolution to XDG/KIO rather than inventing a handler.

The fallback follows freedesktop MIME Applications precedence. User `~/.config/mimeapps.list` retains priority. All other MIME groups are left unchanged.

## Preserved Pass 28 repairs

- XDG_CURRENT_DESKTOP=FlameWM globally, not FlameWM:KDE; KDE MIME defaults provided by the isolated session overlay, not global KDE desktop impersonation.
- XRandR display + panel snapshots captured before native refresh; shell resync checks every panel by output ID, moves/resizes native panel surfaces, then updates rounded masks and popups.
- Brave-specific KDE wallet identity from launcher/.desktop session overrides, KWallet 5/6 session detection and noncredential PAM initializer. Existing cookie databases, Local State, wallet content and user application associations are never edited.

## Actual verification performed here

- **37/37** targeted Python checks passed, including a regression that fails the original inferred Cargo targets and ensures that `flamewm-session` remains a selected binary.
- Python's TOML parser validated the edited manifest; target-discovery rule independently verified from the official Cargo Book.
- Existing XDG MIME query fixture verifies that `~/.config/mimeapps.list` outranks the session's fallback and that a suitable installed handler is chosen.
- New Rust regression test for missing KDE video handler is present **but not executed**.
- **NOT PERFORMED:** Cargo compilation, rustfmt, full Rust tests, Debian build, physical XRandR monitor swapping, Dolphin file activation, or Brave wallet session continuity. The container has no installed `cargo`/`rustc` and no available network; runtime session has neither physical displays nor a user KWallet.

## Apply directly over the full checkout

```sh
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender || exit 1
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_pass29_cargo_targets_mime_monitor_wallet_2026-10-20.tar.gz"
test -f "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
  tar -xzf "$ARCHIVE" -C . && \
  CARGO_BUILD_JOBS=2 ./scripts/check-cargo && \
  CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Do not install an old `.deb` if the build fails. After `FLAMEWM-DEB-READY`, install the freshly built Debian package and perform a fresh login. Validate text/video associations, resynced panels/Start after monitor rearrangement, and Brave logins using the original encrypted profile. Do not reset the wallet or browser profile.
