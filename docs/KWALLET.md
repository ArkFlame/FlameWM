# KDE Wallet interop (Plasma <-> FlameWM)

Contract: switching a user between Plasma and FlameWM must never delete, reset, re-create,
re-encrypt or lock out an existing wallet (`~/.local/share/kwalletd/*.kwl`, `*.salt`, `kwalletrc`).

## What FlameWM does

- Starts no wallet daemon. `kwalletd6` or `kwalletd5` (or `ksecretd`, which owns `org.freedesktop.secrets`)
  start by D-Bus activation from `/usr/share/dbus-1/services` and `$XDG_DATA_HOME/dbus-1/services`.
- Publishes the session identity (`DISPLAY`, `XDG_CURRENT_DESKTOP`, `XDG_DATA_DIRS`, ...) to D-Bus activation
  before children start, so a later-activated wallet daemon sees a valid session. It restores the prior
  activation values on exit.
- Hands the PAM socket (`PAM_KWALLET5_LOGIN` / `PAM_KWALLET_LOGIN`) to `pam_kwallet_init` once, bounded to 3 s.
  No password is synthesized. Missing socket or helper only logs a warning.
- Chooses the wallet generation (`resolve_kwallet_version`): explicit `kwallet5|kwallet6` hint first, then
  installed generation. KDE's `org.kde.kwalletd5.service` may be an alias for `kwalletd6`; it is not counted as a
  KWallet 5 install.
- Portal: `packaging/xdg-desktop-portal/flamewm-portals.conf` sets `Secret=kwallet`. Never gnome-keyring.
- Reads `kwalletrc` and wallet names read-only (`flamewm-applications`). Never writes them.

## What FlameWM never does

- Never runs `rm`, `unlink` or `remove_*` on `~/.local/share/kwalletd`, `kwalletrc`, or `~/.local`/`~/.config` roots.
  The only user-file removal in the compat layer deletes a FlameWM-owned Brave overlay marked by its own header.
- Packaging has no maintainer scripts (`postinst`/`prerm`/`purge`). `apt purge` and `pacman -R` never touch user data.
- Never starts `gnome-keyring-daemon` or ships a `pam_gnome_keyring` module, which would shadow
  `org.freedesktop.secrets` and create a second, empty keyring.

## Packages

- Debian: `kwallet6`, `libpam-kwallet5` in `Suggests` (`packaging/debian/control.in`).
- Arch: `kwallet6`, `kwallet-pam` in `optdepends` (`packaging/arch/PKGBUILD`, `.SRCINFO`).
- Rows: `docs/DEPENDENCIES.md`.

## Verify

```sh
python3 -I tools/tests/test_kwallet_contract.py
python3 -I tools/test_flamewm_package_dependencies.py
```

Static tests prove the repo contract only. Real unlock, Brave/Telegram credential reads and
Plasma-to-FlameWM logout continuity need a non-sensitive test profile on the user's machine. Do not
delete `~/.local/share/kwalletd` to "reset" a test.
