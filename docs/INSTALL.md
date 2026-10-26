# Install

FlameWM installs two sessions from one package: **FlameWM** (X11), which is the supported default
and needs the X11, Xft, and Fontconfig runtime libraries, and **FlameWM (Wayland)**, a preliminary
compositor session that is not yet validated on real hardware. Neither session replaces an existing
display manager; both are selected from its session chooser.

Build and package on an x86_64 Linux machine with the Rust toolchain and
FlameWM build dependencies installed:

```bash
./scripts/build-deb
```

`./scripts/build-deb` compiles the workspace, packages it, checks the actual
`amd64` Debian metadata, verifies all release checksums, and confirms both
session entries (`/usr/share/xsessions/flamewm.desktop` and
`/usr/share/wayland-sessions/flamewm-wayland.desktop`). It removes any previous
`flamewm_VERSION_amd64.deb` before building, so a failed operation cannot leave
an older invalid `Architecture: any` package at the advertised location. Run
**this one command on the full checkout**, not a `test -f PATCH && ...` command
chain.

It refuses to package a compositor that cannot open a tty: the build preflights
the `libinput`, `gbm`, `libdrm` and `libudev` development files plus an
`Xwayland` binary on `PATH` (nothing is installed by the script), and then
requires `flamewm-wayland --capabilities` to print `drm_compiled=true`.

The command refuses incomplete release directories. It writes reproducible
`flamewm-VERSION-source.tar.gz` and `flamewm-VERSION-linux-x86_64.tar.gz`
archives plus `SHA256SUMS` under `.target/dist`. When `dpkg-deb` is installed,
it also writes `flamewm_VERSION_amd64.deb`.

Install the Debian artifact with your normal package manager, or unpack the
Linux archive under `/` as root. Select `FlameWM` from your display manager's
session chooser to log in to the X11 session. `FlameWM (Wayland)` is also
registered; it is preliminary, so read [Wayland session](#wayland-session-preliminary)
below before selecting it.

## Display-manager session registration

The `.deb` installs two session entries:

| Session | File | `Exec` |
| --- | --- | --- |
| `FlameWM` (X11) | `/usr/share/xsessions/flamewm.desktop` | `/usr/libexec/flamewm/flamewm-session-wrapper` |
| `FlameWM (Wayland)` | `/usr/share/wayland-sessions/flamewm-wayland.desktop` | `/usr/libexec/flamewm/flamewm-wayland-session --backend=drm` |

After installing or upgrading the package, log out and select **FlameWM**
from the login screen's session chooser. Verify with:

```bash
ls -l /usr/share/xsessions/flamewm.desktop /usr/share/wayland-sessions/flamewm-wayland.desktop
command -v flamewm-session flamewm-wayland
```

The X11 session is the supported default and stays the recommended login. The
Wayland session is a real compositor (`crates/flamewm-wm-wayland`), not a
re-registered X11 entry, but its native DRM/LightDM login has **not** been
validated on real hardware yet.

### Wayland session (preliminary)

The session wrapper starts `flamewm-wayland` and only continues when
`flamewm-wayland --capabilities` reports `drm_compiled=true`; a build without
the DRM feature exits 2 with `stage=capabilities status=fail
detail=drm_feature_missing`. The nested developer backend (`--backend=nested`)
is never used by the session entry.

Manual test procedure, unvalidated on this host:

1. Install the `.deb` while logged into a working X11 session.
2. Run `flamewm-wayland --capabilities` and confirm the output is
   `flamewm-capabilities=1`, `drm_compiled=true`, `backend=native`.
3. Log out, select `FlameWM (Wayland)` in LightDM, and log in.
4. Collect `~/.cache/flamewm/wayland-session.log` (mode 0600, previous run in
   `wayland-session.log.1`) and `journalctl -b`. The
   `FLAMEWM-WAYLAND-STARTUP stage=... status=... code=...` lines name the stage
   that failed.
5. On success verify: first frame on the connector, panel and shell visible, a
   terminal opens and accepts input, pointer and keyboard work, Ctrl+Alt+Fn VT
   switch away and back, and a further login after the VT return.
6. If the login drops back to the greeter, select `FlameWM` (X11) to recover.
   The Wayland wrapper does not fall back to X11 by itself.

`flamewm-wayland --probe` is an additional read-only advisory report: it never
takes the seat and never changes the session exit code.

A login that returns to the greeter after showing a text cursor has been
reported and is still under investigation; no successful hardware login of the
native backend is on record.

The desktop **Convert** wheel runs ImageMagick (`magick` or `convert`) as an
optional subprocess. The Debian package recommends `imagemagick`; when it is
absent, install it with `sudo apt install imagemagick`. Exact AVIF/HEIC format
support depends on the installed ImageMagick delegates and security policy.

## Install the `.deb` on another computer (SDDM)

The build creates `.target/dist/flamewm_0.1.2_amd64.deb`. Copy it to the
other x86_64 Ubuntu/Debian computer, then run:

```bash
sudo apt install ./flamewm_0.1.2_amd64.deb
```

The package **automatically installs** `/usr/share/xsessions/flamewm.desktop`
with `Exec=/usr/libexec/flamewm/flamewm-session-wrapper`, and the Wayland entry
at `/usr/share/wayland-sessions/flamewm-wayland.desktop`. SDDM discovers
sessions from both directories; no manual session registration, symlink, or
`update-grub` is needed. Log out (or restart SDDM when safe) and choose
**FlameWM** in the SDDM session selector. An X11-capable SDDM/Xorg login
environment must already be installed on the destination; this package does not
install SDDM itself.

Validate the package before copying:

```bash
dpkg-deb --contents .target/dist/flamewm_0.1.2_amd64.deb | grep -E '/xsessions/flamewm.desktop|/wayland-sessions/flamewm-wayland.desktop|/usr/bin/flamewm-session|/usr/bin/flamewm-wayland'
python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.2_amd64.deb
```

FlameWM ships an X11 session and a preliminary Wayland session. Only the X11
session is currently supported; the Wayland session awaits hardware validation.


### Troubleshooting a rejected `.deb`

Verify the actual `Architecture` field before attempting installation:

```bash
dpkg --print-architecture
dpkg-deb -f .target/dist/flamewm_0.1.2_amd64.deb Architecture
```

Both should print `amd64` on an x86_64 Debian/Ubuntu computer. If the
package says `any`, it is an old invalid artifact. Rebuild using the current
`scripts/package`, then transfer the newly generated file. On the target
computer, install from the location where it was copied, e.g.
`sudo apt install ./flamewm_0.1.2_amd64.deb` in that directory.


If installing the package directly from a restricted external mount produces
an `_apt` sandbox access notice, copy it to a publicly traversable directory
first (this does not require weakening your home-directory permissions):

```bash
cp .target/dist/flamewm_0.1.2_amd64.deb /tmp/flamewm_0.1.2_amd64.deb
chmod 0644 /tmp/flamewm_0.1.2_amd64.deb
sudo apt install /tmp/flamewm_0.1.2_amd64.deb
```

If a previous failed installation left unrelated dependencies unpacked, use
`sudo dpkg --configure -a` to finish their configuration before retrying.
A successful build alone does not imply that `./scripts/package` completed;
verify the output `.deb` architecture and `SHA256SUMS` above.
