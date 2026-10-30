# Install

FlameWM installs two sessions from one package: **FlameWM** (X11), the default, which needs the
X11, Xft, and Fontconfig runtime libraries, and **FlameWM (Wayland)**. Neither session replaces an
existing display manager; both are selected from its session chooser.

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
registered; read [Wayland session](#wayland-session) below before selecting it.

## Arch Linux

Arch Linux builds a native pacman package from `packaging/arch/PKGBUILD`. From the full git
checkout on an x86_64 Arch system with `base-devel` installed:

```bash
./scripts/build-arch --syncdeps
sudo pacman -U .target/arch/packages/flamewm-git-<pkgver>-1-x86_64.pkg.tar.zst
```

`./scripts/build-arch` runs `makepkg --cleanbuild` in `.target/arch` against the **committed**
`HEAD` of the checkout (uncommitted changes are reported and not packaged). Its arguments go to
`makepkg`: `--syncdeps` installs the build and runtime dependencies with pacman, `--nocheck` and
`--install` work as usual. It prints `FLAMEWM-ARCH-READY` only after it has checked the package
version against `scripts/version`, the session entries, the `/usr/lib/flamewm` helpers and the
Arch PAM service, and has run `namcap` when it is installed. Without a checkout, the same
PKGBUILD builds upstream `main` like any VCS package: `cd packaging/arch && makepkg -si`.

The package is `flamewm-git` (`provides=flamewm`). It follows the Arch package guidelines, so its
layout differs from the `.deb` in three places:

| Item | Debian | Arch |
| --- | --- | --- |
| Session helpers | `/usr/libexec/flamewm/` | `/usr/lib/flamewm/` |
| PAM service `/etc/pam.d/flamewm-lock` | `@include common-auth` / `common-account` | `include system-auth` |
| Third-party license texts | `/usr/share/doc/flamewm/LICENSES` | also `/usr/share/licenses/flamewm-git/` |

The session entries are the same files with the helper path rewritten:
`/usr/share/xsessions/flamewm.desktop` runs `/usr/lib/flamewm/flamewm-session-wrapper` and
`/usr/share/wayland-sessions/flamewm-wayland.desktop` runs
`/usr/lib/flamewm/flamewm-wayland-session --backend=drm`. LightDM, SDDM and GDM read both
directories; no extra registration is needed. For LightDM install `lightdm` plus a greeter (for
example `lightdm-gtk-greeter`) and enable it with `sudo systemctl enable lightdm`.

Upgrade by building again and installing the newer package with `sudo pacman -U`. pacman keeps
local edits of the two configuration files (`/etc/pam.d/flamewm-lock`,
`/etc/xdg/menus/flamewm-applications.menu`); a changed upstream copy is written next to them as
`.pacnew`. Uninstall with `sudo pacman -R flamewm-git`; edited configuration files are kept as
`.pacsave`. pacman never touches per-user data: `~/.config/flamewm`, `~/.local/share/flamewm` and
`~/.cache/flamewm` stay in place after an upgrade or removal.

Optional integrations (xdg-utils, xrandr, dunst, dex, udisks2, ImageMagick, ffmpeg, IBM Plex Sans,
polkit agent, portal backends, kwallet-pam, and `libxkbcommon-x11` for `flamewm-wayland
--backend=nested` inside an X11 desktop) are listed as `optdepends` and printed by pacman. The
package bundles the McMojave cursor theme, so it conflicts with the AUR `mcmojave-cursors`
packages that install the same `/usr/share/icons/McMojave-cursors` directory.

The package has been built with `makepkg` in a clean Arch container, installed, upgraded and
removed with pacman, and both sessions were started from it on Xvfb/Xephyr (the Wayland
compositor through its nested backend). A real display-manager login and the DRM/KMS Wayland
backend on hardware are not verified yet, the same open items as on Debian.

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

### Wayland session

The session wrapper starts `flamewm-wayland` and only continues when
`flamewm-wayland --capabilities` reports `drm_compiled=true`; a build without
the DRM feature exits 2 with `stage=capabilities status=fail
detail=drm_feature_missing`. The nested developer backend (`--backend=nested`)
is never used by the session entry.

Manual test procedure (not yet run on real hardware):

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
optional subprocess. The Debian package suggests `imagemagick`; when it is
absent, install it with `sudo apt install imagemagick`. Exact AVIF/HEIC format
support depends on the installed ImageMagick delegates and security policy.

## Optional: Hermes model proxy

`flamewm-assistant` (Super+C) chats with ARK AI by default. If a Hermes model proxy is already
running on `127.0.0.1:8645`, its models appear under a Hermes group in the model menu. FlameWM does
not install or start Hermes. Speech input and output are off until enabled in Settings. See
[`ASSISTANT.md`](ASSISTANT.md) for the providers.

## Assistant speech (optional)

Speech is off by default. Turning on speech to text or text to speech in Settings > Assistant starts
**Set up voice** in the background (`flamewm-assistant --voice-setup`), which does the whole setup in
one step. It retries up to three times (60 s apart) and records progress and the final state, or the
reason for a failure, in `voice/status`. Pressing the mic or reading a reply aloud before it finishes
shows the same setup in the assistant window. Setup is idempotent: a finished setup is not repeated,
and a second setup waits for the first one's lock instead of running alongside it:

1. Creates a private virtual environment at `$XDG_DATA_HOME/flamewm/assistant/voice/venv` (default
   `~/.local/share/flamewm/assistant/voice/venv`). It needs `python3` 3.10 or newer with `venv`
   support. No root and no system-wide packages.
2. Installs the CPU build of PyTorch from `download.pytorch.org`, then `ptts` (text to speech) and
   `fermion-research` (speech to text) from PyPI.
3. Downloads the pinned text-to-speech model into `voice/models/tts-phonon-local` through the model
   store. Each file is checked by size and SHA-256 before use.
4. Downloads the speech-to-text model Phonon-2 into `voice/stt-cache`. `fermion-research` fetches it and
   checks it against its own pinned SHA-256.
5. Writes `voice/bin/speak` and `voice/bin/transcribe`. They are used when `synthesize_command` or
   `transcribe_command` is not set in `assistant-speech.conf`. No manual configuration is needed.

Disk use is about 1.8 GB: venv 1.4 GB, speech-to-text cache 0.33 GB, text-to-speech model 0.06 GB.
Deleting the `voice` directory removes the whole feature.

Optional system package: `python3-venv` (Debian or Ubuntu: `sudo apt install python3-venv`). Setup stops
with this hint when it is missing. See `docs/DEPENDENCIES.md`.

| Role | Model | Pinned files (commit) | Total size | License | Gated |
|---|---|---|---|---|---|
| Speech-to-text | `FermionResearch/Phonon-2` | fetched and verified by `fermion-research` | about 164 MB | CC-BY-4.0 weights; Apache-2.0 code | no |
| Text-to-speech | `gradium/phonon-31m-en` | `model.q8.gguf`, `config.json`, `tokenizer.json`, `default-voice.safetensors` (`92741c5b`) | 65,051,089 B | Apache-2.0 | no |

SHA-256 of the pinned text-to-speech files (the content hash of a local download for the non-LFS files):

- `model.q8.gguf` `a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae`
- `config.json` `b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85` (4,634 B)
- `tokenizer.json` `691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936` (277,967 B)
- `default-voice.safetensors` `dc537478eaadc1a5257fcda2e3501ab81e14c111a5d95a5838f350db2afd0f1a` (256,136 B)

Tested here, on Linux x86-64 CPU (Intel i5-11400H): the live setup ran end to end. The text-to-speech output
was a 24 kHz mono WAV, and speech-to-text transcribed it back to the same sentence.

Limits:

- Only Linux x86-64 CPU is tested. Arm Linux has a runtime library but is untested. Apple silicon uses MLX
  and is not wired up. There is no GPU path.
- Speech-to-text was tested on a short generated clip, not on long recordings.
- The voice directory path must not contain spaces, because command templates split on whitespace. Setup
  refuses such a path.
- Setup needs network access to `pypi.org`, `files.pythonhosted.org`, `download.pytorch.org` and
  `huggingface.co`. A failed step names the step and the fix; rerunning setup resumes.
- `record_command` and `play_command` stay `arecord` and `aplay`.
- `gradium/phonon-40m-*` (French, German, Spanish, Portuguese and multilingual) are CC-BY-NC-4.0 and gated.
  They are not pinned and are not for commercial use.
- Optional system tools: `curl` (model downloads from the store). No package declares it.

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

FlameWM ships an X11 session and a Wayland session. X11 is the default; no
Wayland login on real hardware is on record yet.


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

## Portable .deb build

Build the .deb on Debian 12 so it runs on Debian 12+, MX, Ubuntu 22.04+: `docker build -t flamewm-deb-builder:bookworm -f packaging/debian/builder.Dockerfile packaging/debian`, then run `./scripts/build-deb` inside it as your user (mount the repo and a `CARGO_TARGET_DIR`). Building on a newer distro raises the glibc requirement.
