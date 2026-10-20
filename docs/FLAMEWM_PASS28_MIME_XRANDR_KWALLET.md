# FlameWM 0.1.0 — Pass 28 (three-issue repair)

Source baseline: uploaded `FlameWM_0.1.0_pass27_kde_mime_kwallet_monitor_layout_meta_drag_2026-10-20.tar(1).gz`.

## 1. Dolphin .txt/.mp4 asks Open With, then monitor layout changes

Pass 27 exported `XDG_CURRENT_DESKTOP=FlameWM:KDE`. This gives KDE-specific MIME defaults but also permits KDE-only autostart/service behavior to be selected, potentially including competing display layout management. A direct causal link to KScreen on the affected physical machine still requires runtime logs; this is a verified architectural risk, not a proven process trace.

**Pass 28:** the session now advertises `FlameWM` only. At login the session helper constructs an **owned, per-session XDG_CONFIG_DIRS fallback** at `~/.config/flamewm/session-compat/config/flamewm-mimeapps.list`, using KDE default associations in XDG precedence order, then filling missing `text/plain` with an installed Kate desktop entry and `video/mp4` with an installed video player. `$XDG_CONFIG_HOME/mimeapps.list` and user desktop-specific preferences retain higher priority. The helper never modifies KDE or user generic MIME files. KDE-first portal selection remains packaged. MIME availability is capability-dependent: no handler is invented if none is installed.

## 2. Moving monitors leaves taskbars, Start or desktop in stale coordinates

**Proven source-level cause:** `crates/flamewm-wm-x11/src/runtime.rs` took `display_before` **after** its XRandR refresh. It consequently missed changes and could omit `DisplaysSnapshotChanged`; the changed PanelService also lacked an unconditional comparison-based signal on RandR refresh. Now both snapshots are captured before handling topology, and display and panel signals are emitted after refresh when changed.

`flamewm-shell` now detects drift for *all* panel output IDs (including changes with unchanged revisions), preserves per-output panel edges during resync, moves native windows before regenerating their rounded native masks, then reprojects the relevant panels. Popup opening uses the reconciled geometry rather than refusing indefinitely on a stale panel rectangle.

## 3. Brave loses encrypted login state or prompts for a different wallet

Chromium/Brave Linux OSCrypt depends on selecting the same encryption backend as the existing profile. A wallet prompt is not proof the correct wallet and key were selected. Pass 28 detects the installed KWallet generation and gives `XDG_CURRENT_DESKTOP=KDE` **only to Brave launches**, via (a) FlameWM's launcher and (b) generated copies of installed Brave `.desktop` entries under a FlameWM session-owned XDG_DATA_DIRS prefix. Existing user `.desktop` overrides remain higher priority; installed files and profiles are untouched. Where SDDM's PAM KWallet module supplied its login credential, the session invokes the installed `pam_kwallet_init` handshake helper to complete unlock without requesting/storing a password. If SDDM has no PAM KWallet hook, the wallet must still be unlocked manually: FlameWM cannot recover the login password from the session.

**Do not** delete `~/.config/BraveSoftware`, rename `Local State`, recreate `kdewallet`, reset cookies, switch to `--password-store=basic`, or reset wallet files. If a separate backend previously rewrote encrypted cookies, source changes alone cannot reconstruct deleted ciphertext.

## Verification

- Passed: 35 focused Python contract/standards tests (`test_flamewm_pass2*.py`), including live `xdg-mime` precedence against temporary application entries.
- Rust regression tests added for monitor output-coordinate swaps, mixed panel edges, Brave launcher identity, MIME default selection, and desktop overrides, but **not executed**.
- **Not available:** `cargo` / `rustc` in patch-authoring container; therefore `cargo check --workspace --all-targets`, `./scripts/build-deb`, KDE/Brave wallet access, and live dual-monitor visual/interaction testing are **unverified**.
- The source archive is a Pass 27 overlay and does not contain the large `assets/` directory; use it on top of the **complete FlameWM checkout** that already has those runtime assets.

## Applying to the existing full FlameWM checkout

```sh
cd /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender
ARCHIVE="$HOME/Downloads/FlameWM_0.1.0_pass28_mime_monitors_brave_kwallet_2026-10-20.tar.gz"
test -f "$ARCHIVE" && tar -tzf "$ARCHIVE" >/dev/null && \
  tar -xzf "$ARCHIVE" -C . && \
  CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets && \
  CARGO_BUILD_JOBS=2 ./scripts/build-deb
```

Only after successful build and `FLAMEWM-DEB-READY`:

```sh
sudo apt install --reinstall ./.target/dist/flamewm_0.1.0_amd64.deb
```

Log out of KDE/FlameWM and start a **fresh FlameWM SDDM session**. Completely close Brave before changing sessions: an already-running Brave process keeps its old OSCrypt backend/environment.

Runtime checks (needed before classifying the three bugs as fixed on hardware):

```sh
printf '%s\n' "$XDG_CURRENT_DESKTOP" "$KDE_SESSION_VERSION"
xdg-mime query default text/plain
xdg-mime query default video/mp4
xrandr --query
pgrep -af 'kscreen|kwallet|brave' || true
```

Expected desktop name: `FlameWM`. Verify Kate / installed video player open without a chooser, monitor movement preserves panel/start positions, and Brave retains **the same** logins between Plasma and FlameWM. KWallet automatic unlock depends on real SDDM/PAM integration; check it on the physical system before shipping.
