# Install

FlameWM requires an X11 session and runtime libraries for X11, Xft, and
Fontconfig. It does not start a desktop session or replace an existing display
manager.

Build and package on an x86_64 Linux machine with the Rust toolchain and
FlameWM build dependencies installed:

```bash
./scripts/build-deb
```

`./scripts/build-deb` compiles the workspace, packages it, checks the actual
`amd64` Debian metadata, verifies all release checksums, and confirms the X11
SDDM session entry. It removes any previous `flamewm_VERSION_amd64.deb` before
building, so a failed operation cannot leave an older invalid `Architecture:
any` package at the advertised location. Run **this one command on the full
checkout**, not a `test -f PATCH && ...` command chain.

The command refuses incomplete release directories. It writes reproducible
`flamewm-VERSION-source.tar.gz` and `flamewm-VERSION-linux-x86_64.tar.gz`
archives plus `SHA256SUMS` under `.target/dist`. When `dpkg-deb` is installed,
it also writes `flamewm_VERSION_amd64.deb`.

Install the Debian artifact with your normal package manager, or unpack the
Linux archive under `/` as root. Select `FlameWM` from your display manager's
session chooser, then log in to an X11 session.

## Display-manager session registration

The `.deb` installs the real X11 session entry at
`/usr/share/xsessions/flamewm.desktop` (`Exec=/usr/bin/flamewm-session`).
After installing or upgrading the package, log out and select **FlameWM**
from the login screen's session chooser. Verify with:

```bash
ls -l /usr/share/xsessions/flamewm.desktop
command -v flamewm-session
```

FlameWM is currently a **native X11** desktop/window manager. There is no
Wayland compositor/provider in this source tree, so this package intentionally
does **not** install a nonfunctional `wayland-sessions/flamewm.desktop` entry.
Do not select a Wayland session expecting this X11 WM to run as a Wayland
compositor. Implement and native-test a Wayland backend before registering it.

The desktop **Convert** wheel runs ImageMagick (`magick` or `convert`) as an
optional subprocess. The Debian package recommends `imagemagick`; when it is
absent, install it with `sudo apt install imagemagick`. Exact AVIF/HEIC format
support depends on the installed ImageMagick delegates and security policy.

## Install the `.deb` on another computer (SDDM)

The build creates `.target/dist/flamewm_0.1.0_amd64.deb`. Copy it to the
other x86_64 Ubuntu/Debian computer, then run:

```bash
sudo apt install ./flamewm_0.1.0_amd64.deb
```

The package **automatically installs** `/usr/share/xsessions/flamewm.desktop`
with `Exec=/usr/bin/flamewm-session`. SDDM discovers X11 sessions from this
directory; no manual session registration, symlink, or `update-grub` is needed.
Log out (or restart SDDM when safe) and choose **FlameWM** in the SDDM
session selector. An X11-capable SDDM/Xorg login environment must already be
installed on the destination; this package does not install SDDM itself.

Validate the package before copying:

```bash
dpkg-deb --contents .target/dist/flamewm_0.1.0_amd64.deb | grep -E '/xsessions/flamewm.desktop|/usr/bin/flamewm-session'
python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.0_amd64.deb
```

FlameWM currently ships an X11 session, **not** a native Wayland compositor.


### Troubleshooting a rejected `.deb`

Verify the actual `Architecture` field before attempting installation:

```bash
dpkg --print-architecture
dpkg-deb -f .target/dist/flamewm_0.1.0_amd64.deb Architecture
```

Both should print `amd64` on an x86_64 Debian/Ubuntu computer. If the
package says `any`, it is an old invalid artifact. Rebuild using the current
`scripts/package`, then transfer the newly generated file. On the target
computer, install from the location where it was copied, e.g.
`sudo apt install ./flamewm_0.1.0_amd64.deb` in that directory.


If installing the package directly from a restricted external mount produces
an `_apt` sandbox access notice, copy it to a publicly traversable directory
first (this does not require weakening your home-directory permissions):

```bash
cp .target/dist/flamewm_0.1.0_amd64.deb /tmp/flamewm_0.1.0_amd64.deb
chmod 0644 /tmp/flamewm_0.1.0_amd64.deb
sudo apt install /tmp/flamewm_0.1.0_amd64.deb
```

If a previous failed installation left unrelated dependencies unpacked, use
`sudo dpkg --configure -a` to finish their configuration before retrying.
A successful build alone does not imply that `./scripts/package` completed;
verify the output `.deb` architecture and `SHA256SUMS` above.
