# FlameWM Pass 27 — Plasma interoperability and native interactions

- Session desktop identity is `FlameWM:KDE` (unless `FLAMEWM_KDE_COMPAT=0`) to consult KDE-specific user MIME defaults in `kde-mimeapps.list` without replacing the user's files. FlameWM remains the primary desktop identity.
- `flamewm-session` imports launch identity and display environment into D-Bus activated services before starting WM/desktop/shell. The .deb includes `flamewm-portals.conf` preferring KDE OpenURI/FileChooser, GTK as fallback.
- Chromium/Brave KDE wallet selection ALSO requires `KDE_SESSION_VERSION=6` for KWallet6 (or 5 for KWallet5). FlameWM now advertises the installed/inherited KDE wallet version when found; without it, Chromium may select KDE4 even though XDG_CURRENT_DESKTOP mentions KDE. Chromium/Brave password storage is NOT the same thing as cookies. Automatic backend inference will now see KDE compatibility, but no password/cookie files are accessed/migrated. If the browser still uses the wrong encrypted store, inspect `brave://version` and use KDE's wallet status; a `--password-store=kwallet6` override should be tested on the EXISTING profile only after backing it up. KDE Wallet unlock needs the real SDDM+PAM session (not forced wallet recreation).
- Meta+left-click now delegates to the existing WM titlebar move reducer; no competing window movement engine is created.
- The portal picker itself is not evidence that a KScreen/XRandR backend changed monitor positions. Trace `xrandr --verbose`, session logs and `kscreen` on the affected installed session. `XDG_CURRENT_DESKTOP=FlameWM:KDE` may affect KDE service activation, so monitor RandR events during validation.
- All native X11 outcomes require testing on the full checkout; this source export has no Cargo toolchain or complete assets.

## Display arrangement

Settings > Displays uses output cards in physical (RandR x/y) order and accepts a horizontal drag (>40 logical px) or explicit Left/Right commands. The adapter uses the live output ID to resolve the two connected/active outputs and invokes `xrandr --output <selected> --left-of/--right-of <other>` using argv (no shell). Unknown, disabled, pending, or greater-than-two active outputs fail without an XRandR command. Native apply is currently not a reversible control-bus transaction; users should keep a terminal available for recovery (`xrandr --auto`). The selected left/right connector order is persisted atomically in `~/.config/flamewm/display-order` and restored at login only if both connectors are active; primary designation and mode are untouched.

## Live verification and recovery

1. Log out of the existing session and log back in through SDDM: editing environment variables while Brave, Dolphin, KWallet, and user-D-Bus services remain running does not change already-running applications.
2. Compare `XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`, `KDE_SESSION_VERSION`, `xdg-mime query default text/plain` inside the *actual* FlameWM session with Plasma. Inspect `~/.config/mimeapps.list` and `~/.config/kde-mimeapps.list` only; do not overwrite either automatically.
3. Check `dpkg -l kwallet6 libpam-kwallet5 xdg-desktop-portal-kde x11-xserver-utils`. Do not delete Brave's Local State, profile, wallet, or password store to fix a decryption-backend mismatch. Existing Brave processes must be fully terminated before comparing.
4. Change monitor order via Settings > Displays; the UI writes a validated connector pair at `~/.config/flamewm/display-order`. To opt out, rename or remove **only this FlameWM-specific preference file**. If a misconfigured layout requires terminal recovery, `xrandr --auto` may restore a usable baseline (check `xrandr --query` first).
5. Test Meta+left-click inside a Dolphin/Brave client, then verify ordinary left click still works, and Meta+right-click does not change normal context-menu behavior.
6. If a file chooser still disrupts RandR, capture `xrandr --query` before and after and inspect KScreen process ownership. KDE compatibility can be disabled by launching a session with `FLAMEWM_KDE_COMPAT=0`; MIME defaults and Brave KWallet inference may then revert.

**Known unverified limits:** native `cargo check`/real X11 runtime were not available in the authoring environment; the arrangement adapter handles exactly two active monitors; the KWallet session PAM chain cannot be proven correct without checking SDDM and installed packages. No browser credentials or KDE/Plasma configuration were modified.
