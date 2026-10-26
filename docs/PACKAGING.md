# Packaging

`./scripts/package` is packaging-only. It never runs Cargo, compiles sources,
or runs tests. It validates that current executable release artifacts exist in
`.target/release`, copies files according to `packaging/install-manifest.txt`,
and stages `flamewm-VERSION` under `.target/dist`.

Archive metadata is normalized with sorted entries, numeric owner/group zero,
and `SOURCE_DATE_EPOCH` (default `0`). The reproducible outputs are
`flamewm-VERSION-source.tar.gz`, `flamewm-VERSION-linux-x86_64.tar.gz`, and
`SHA256SUMS` under `.target/dist`.

If `dpkg-deb` exists, `flamewm_VERSION_amd64.deb` is emitted beside the
archives. Debian metadata comes from `packaging/debian/control.in` and
`copyright`. Absence of `dpkg-deb` is allowed; missing release binaries or
package sources are errors. `SHA256SUMS` covers every artifact that was
emitted.


## Multi-call applications

`flamewm`, `flamewm-desktop`, `flamewm-shell`, `flamewm-settings`, `flamewm-explorer` and
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
`/usr/share/wayland-sessions/flamewm-wayland.desktop` for the preliminary
Wayland session (see [`WAYLAND_PLAN.md`](WAYLAND_PLAN.md)); the X11 session is
the supported default.

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

## Wayland session packaging (preliminary)

`scripts/build-deb` builds with `--features flamewm-wm-wayland/drm` and
`scripts/dev-deb` uses the same feature list with the `fast` profile, so a
packaged compositor always carries the native DRM backend. Two fail-closed
gates protect the login entry: the preflight (`pkg-config --exists libinput gbm
libdrm libudev` plus an `Xwayland` binary on `PATH`, nothing installed) and the
post-build `flamewm-wayland --capabilities` check for `drm_compiled=true`.

`packaging/debian/control.in` adds the Wayland runtime set on top of the
existing X11 dependencies: `libgbm1, libdrm2, libinput10, libudev1, libegl1,
libgles2, xwayland`. Seat access is implemented against systemd-logind over
D-Bus, so no seat library (`libseat`) and no `seatd` is packaged or required.

The Wayland session itself is preliminary and unvalidated on hardware. See
[`WAYLAND_PLAN.md`](WAYLAND_PLAN.md) for the implementation, the diagnostics and
the list of unverified items.

## Imported source archives and stale Cargo compiled APIs

A source archive extracted on an existing checkout may restore file timestamps older than
previous compiled artifacts. Bare `cargo check` can then reuse an incompatible desktop-core rlib
and produce `E0432/E0599/E0061` even when current source exports the Convert API.
Use `./scripts/check-cargo` for a quick development check and `./scripts/build-deb` for a
release. Both validate the source contract and forcibly clean only the desktop core + desktop
Cargo outputs in their own profiles before compiling. `build-deb` cleans the old named `.deb`
before doing anything and never prints `FLAMEWM-DEB-READY` on failure.
