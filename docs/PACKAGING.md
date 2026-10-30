# Packaging

`./scripts/package` is packaging-only. It never runs Cargo, compiles sources,
or runs tests. It validates that current executable release artifacts exist in
`.target/release`, copies files according to `packaging/install-manifest.txt`,
and stages `flamewm-VERSION` under `.target/stage`, so `.target/dist` holds only
artifacts meant for upload.

Archive metadata is normalized with sorted entries, numeric owner/group zero,
and `SOURCE_DATE_EPOCH` (default `0`). The reproducible outputs are
`flamewm-VERSION-source.tar.gz`, `flamewm-VERSION-linux-x86_64.tar.gz`, and
`SHA256SUMS` under `.target/dist`.

If `dpkg-deb` exists, `flamewm_VERSION_amd64.deb` is emitted beside the
archives. Debian metadata comes from `packaging/debian/control.in` and
`copyright`. Absence of `dpkg-deb` is allowed; missing release binaries or
package sources are errors. `SHA256SUMS` covers every artifact that was
emitted.

## Distribution directory

Every packaging lane publishes into `.target/dist`, and nothing else is written there:

| Lane | Command | Artifacts |
| --- | --- | --- |
| Debian | `./scripts/build-deb` | `flamewm_VERSION_amd64.deb`, `flamewm-VERSION-source.tar.gz`, `flamewm-VERSION-linux-x86_64.tar.gz` |
| Arch | `./scripts/build-arch` | `flamewm-git-VERSION.rN.gHASH-1-x86_64.pkg.tar.zst` |
| cpak | `./scripts/build-cpak` | `flamewm-terminal-VERSION-cpak.oci.tar`, plus `flamewm-terminal-VERSION-cpak.json` with `--push` |

Each lane replaces only its own artifacts, then `./scripts/dist-checksums` rewrites
`SHA256SUMS` over everything in the directory. `./scripts/build-dist` runs every lane whose tools
are installed (or the lanes named by `--deb`, `--arch`, `--cpak`) and verifies the checksums.
`.github/workflows/dist.yml` runs the three lanes on GitHub and uploads them together as the
`flamewm-dist` artifact; it can be started by hand and is reused by the release workflow.

## cpak application image

`./scripts/build-cpak` exports the committed `HEAD` with `git archive`, builds
`packaging/cpak/Containerfile` with podman, docker or buildah, checks that `flamewm-terminal` and
`flamefetch` run in the image and saves it as an OCI archive. The Containerfile compiles the
workspace inside `archlinux:base-devel`, stages it with `scripts/package --stage`, and
`tools/cpak_package.py prune` removes the host login sessions and PAM service, so the runtime image
always matches the glibc it was built against. `--push` publishes `IMAGE:VERSION` (default
`ghcr.io/arkflame/flamewm-terminal`, or `--image NAME`) and writes the cpak v3 manifest pinned to
the digest the registry reported. See `packaging/cpak/USAGE.md` for installing it with cpak.


`./scripts/package --stage DESTDIR` runs only the install step: it validates the release
binaries and packaging sources and installs the runtime tree into an empty or absent `DESTDIR`,
without archives, `.deb` or checksums. `--libexecdir DIR` (default `/usr/libexec/flamewm`) moves
the session helpers and rewrites the `Exec` of both session entries, and `--pam FILE` selects the
PAM service file. Both options are rejected outside `--stage`, so the Debian layout cannot change.
Distribution packages call this mode instead of repeating the layout.

## Arch Linux package

`packaging/arch/PKGBUILD` builds `flamewm-git` with the Rust package guidelines
(`cargo fetch --locked` in `prepare()`, `cargo build --frozen --release --workspace --features
flamewm-wm-wayland/drm` in `build()`) and installs with `scripts/package --stage "$pkgdir"
--libexecdir /usr/lib/flamewm --pam packaging/arch/flamewm-lock.pam`. `build()` keeps the
`drm_compiled=true` capability gate of `build-deb`. `CARGO_PROFILE_RELEASE_STRIP=false` leaves
stripping and the `-debug` split package to makepkg's `strip`/`debug` options. `pkgver()` is the
workspace version plus `r<commit count>.g<hash>`, so it always starts with `scripts/version`.
`backup` covers `/etc/pam.d/flamewm-lock` and `/etc/xdg/menus/flamewm-applications.menu` (the
Debian `conffiles`). Icon and desktop database refreshes come from the pacman hooks of
`gtk-update-icon-cache` and `desktop-file-utils`; the package ships no `.install` script and no
service.

`./scripts/build-arch` points a copy of the PKGBUILD at the checkout's committed `HEAD`
(`git+file://...#commit=`), runs `makepkg --cleanbuild --force` under `.target/arch` (sources,
build tree and packages stay there), copies the verified package into `.target/dist` and verifies the produced package: version prefix, session
entries, helpers in `/usr/lib/flamewm`, no `/usr/libexec`, and the Arch PAM service, followed by
`namcap` when installed. `.github/workflows/arch.yml` runs it in `archlinux:base-devel` on every
push and pull request, then installs the package, checks every installed ELF with `ldd`, starts
the X11 session on Xvfb and removes the package again.

## Multi-call applications

`flamewm`, `flamewm-desktop`, `flamewm-shell`, `flamewm-settings`, `flamewm-explorer`, `flamewm-finder` and
`flamewm-terminal` are installed as hard links to one executable, `/usr/bin/flamewm-apps`
(`crates/flamewm-apps`), which runs the application named by `argv[0]`. The applications link the
same renderer, image decoders, X11 and D-Bus code; as one file the kernel shares those pages
between their processes. Hard links (not symlinks) keep `current_exe()` and every packaged-asset
lookup at the application's own name. Each application crate still builds its standalone binary
for development; `scripts/package` installs only `flamewm-apps` plus the links.

## Debian architecture and failure handling

The binary Debian control `Architecture` is the host's actual `dpkg
--print-architecture` (currently `amd64` for the supported x86_64
release), **never** the source-package placeholder `any`. Packaging refuses
non-x86_64/amd64 hosts rather than mislabeling a foreign-architecture binary.
The source manifest registers executable scripts explicitly, including
`scripts/version`. The source archive is built before the `.deb` so a
manifest failure does not emit a new, apparently installable package.

After `./scripts/package`, run:

```bash
dpkg-deb -f .target/dist/flamewm_0.1.2_amd64.deb Architecture
# Expected: amd64
(cd .target/dist && sha256sum -c SHA256SUMS)
python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.2_amd64.deb
```

Do **not** use `--force-architecture`. Install from the actual `dist` path
(or copy the `.deb` to an accessible location first). The package carries
`/usr/share/xsessions/flamewm.desktop` for the X11 session selector and
`/usr/share/wayland-sessions/flamewm-wayland.desktop` for the Wayland session
(see [`WAYLAND_PLAN.md`](WAYLAND_PLAN.md)); the X11 session is the default.

## One-command fresh Debian build (recommended)

From the full source checkout (with `assets/`), run:

```bash
./scripts/build-deb
```

It rebuilds the current Rust workspace, invokes `./scripts/package`, and
requires all of the following before reporting `FLAMEWM-DEB-READY`:
`Architecture: amd64`, a complete `SHA256SUMS`, the X11 session file
`/usr/share/xsessions/flamewm.desktop`, and the Wayland session file
`/usr/share/wayland-sessions/flamewm-wayland.desktop`. It deletes the old named
`.deb` **before compiling** and treats any failure as fatal. Do not install a
package if `FLAMEWM-DEB-READY` did not appear.
The source package/`.deb` publisher also cleans incomplete release outputs on
failure. For users overlaying a source `.tar.gz` on their checkout, this avoids
relying on the existence of a separately downloaded patch file.

A full Rust toolchain and all FlameWM runtime assets are needed. The smaller
source handoff archives intentionally omit bulky assets; overlay them onto the
original checkout without deleting its `assets/` tree.

## Wayland session packaging

`scripts/build-deb` builds with `--features flamewm-wm-wayland/drm` and
`scripts/dev-deb` uses the same feature list with the `fast` profile, so a
packaged compositor always carries the native DRM backend. Two fail-closed
gates protect the login entry: the preflight (`pkg-config --exists libinput gbm
libdrm libudev` plus an `Xwayland` binary on `PATH`, nothing installed) and the
post-build `flamewm-wayland --capabilities` check for `drm_compiled=true`.

`packaging/debian/control.in` adds the Wayland runtime set on top of the
existing X11 dependencies: `libgbm1, libdrm2, libinput10, libudev1, libegl1,
libgles2, xwayland`. The full classification (Depends, Recommends, Suggests, and
the Arch mapping) is in [`DEPENDENCIES.md`](DEPENDENCIES.md). Seat access is implemented against systemd-logind over
D-Bus, so no seat library (`libseat`) and no `seatd` is packaged or required.

No Wayland login on real hardware is on record yet. See
[`WAYLAND_PLAN.md`](WAYLAND_PLAN.md) for the implementation, the diagnostics and
the list of unverified items.

## Release workflow

`.github/workflows/release.yml` runs when a `v<version>` tag is pushed. It fails
unless the tag matches `scripts/version`, then runs the distribution workflow (Arch package, `.deb` and archives, and the
cpak image, which it pushes to GHCR) and attaches every artifact, the digest-pinned cpak manifest
and a `SHA256SUMS` file to a GitHub release with generated notes.

The same run updates the `flamewm-git` AUR package: it copies
`packaging/arch/PKGBUILD`, regenerates `.SRCINFO` and pushes both. This step is
skipped until a maintainer stores the AUR account's SSH private key as the
`AUR_SSH_PRIVATE_KEY` repository secret. The commit identity comes from the
optional `AUR_GIT_NAME` and `AUR_GIT_EMAIL` repository variables.

To release, bump `workspace.package.version` in `Cargo.toml`, add the
`CHANGELOG.md` entry, merge, then tag the merge commit:

```bash
git tag v0.1.3
git push origin v0.1.3
```

## Imported source archives and stale Cargo compiled APIs

A source archive extracted on an existing checkout may restore file timestamps older than
previous compiled artifacts. Bare `cargo check` can then reuse an incompatible desktop-core rlib
and produce `E0432/E0599/E0061` even when current source exports the Convert API.
Use `./scripts/check-cargo` for a quick development check and `./scripts/build-deb` for a
release. Both validate the source contract and forcibly clean only the desktop core + desktop
Cargo outputs in their own profiles before compiling. `build-deb` cleans the old named `.deb`
before doing anything and never prints `FLAMEWM-DEB-READY` on failure.
