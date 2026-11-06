# Session keyring and browser cookies

Switching between KDE Plasma and FlameWM made Chromium-family browsers lose cookies and logins.

## Root cause

Chromium picks its OSCrypt password store from the desktop environment
(`base/nix/xdg_util.cc` `GetDesktopEnvironment`, `components/os_crypt/sync/key_storage_util_linux.cc` `SelectBackend`).

- `XDG_CURRENT_DESKTOP` is split on `:` and the first recognised token wins (Unity, Deepin, GNOME, X-Cinnamon, KDE, Pantheon, XFCE, UKUI, LXQt, COSMIC). `KDE` plus `KDE_SESSION_VERSION` 5 or 6 selects KWallet 5 or 6.
- `FlameWM` is not recognised, so Chromium falls back to `DESKTOP_SESSION`, which can only ever yield KDE 3/4 (never 5/6) and otherwise ends at `OTHER`. `OTHER` uses the basic-text store with a hardcoded key (`v10` prefix).
- Plasma encrypts with a key held in KWallet (`v11` prefix). Each session cannot decrypt the other's cookies, so they are dropped.
- Plasma also unlocks the wallet with the login password through `plasma-kwallet-pam.service`, which runs `pam_kwallet_init`. That unit is ordered against Plasma units and is never pulled into a FlameWM session.

## Why not `XDG_CURRENT_DESKTOP=FlameWM:KDE`

It would fix Chromium and Electron at once, but FlameWM deliberately never advertises KDE session-wide: KDE-only autostarts (KScreen) compete for RandR ownership once an application activates KDE services. `DESKTOP_SESSION` cannot select KWallet 5/6, so it is not an alternative. The identity is therefore given per launcher.

## Fix

- `flamewm-session-core` `select_keyring` picks the store Plasma would use: KWallet (6 if `kwalletd6`, `ksecretd` or its D-Bus service exists, else 5), then gnome-keyring, else none. `FLAMEWM_BRAVE_PASSWORD_STORE` presets are respected (`basic`/`none` opt out).
- The FlameWM-owned XDG overlay rewrites the Exec lines of Brave, Chromium, Chrome, Vivaldi, Edge and Opera launchers to `env XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=<n>` whenever KWallet is detected. Entries with an explicit `--password-store` or desktop override are left alone; system entries are never modified.
- `pam_kwallet_init` runs once at session start when `PAM_KWALLET5_LOGIN` is a live socket (bounded wait, failures logged).
- Without KWallet, `gnome-keyring-daemon --start --components=pkcs11,secrets,ssh` is started like cosmic-session does. With neither, one log line is written.
- The D-Bus/systemd activation environment keeps `XDG_CURRENT_DESKTOP=FlameWM` and an empty `KDE_SESSION_VERSION`, for the KScreen reason above.

## Limits

Electron apps and browsers started by hand from a terminal are not wrapped. Give them `--password-store=kwallet6` (for example through `~/.config/electron-flags.conf` or `chromium-flags.conf`).
The display manager PAM stack must load `pam_kwallet5.so` (kwallet-pam) for the handoff socket to exist; see `docs/PACKAGING.md`.

## References

- https://github.com/chromium/chromium/blob/main/base/nix/xdg_util.cc
- https://github.com/chromium/chromium/blob/main/components/os_crypt/sync/key_storage_util_linux.cc
- https://github.com/KDE/kwallet-pam (`pam_kwallet_init`, `plasma-kwallet-pam.service.in`)
- https://github.com/pop-os/cosmic-session/blob/master/data/start-cosmic
