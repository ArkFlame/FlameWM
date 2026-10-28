# cpak application packaging

This is an **application-only** cpak v3 package for FlameWM Terminal and
Flamefetch, not a full FlameWM desktop login session. It uses cpak's isolated
`displayX11` support; it does not receive the host's X11 display.

The native DRM/Wayland desktop uses raw logind system-bus calls. cpak v3 has
no raw system bus permission, so advertising a working native host-login session
here would be misleading. Likewise, the full Shell expects desktop-service
D-Bus integration absent from the isolated package. Keep the `.deb` and Arch
packages for full sessions.

## Build

```sh
./scripts/build-cpak
```

The script needs podman, docker or buildah. It builds the committed `HEAD`
inside `archlinux:base-devel` (see `Containerfile`), so no host Rust build or
glibc match is involved, checks both binaries in the finished image and writes
`.target/dist/flamewm-terminal-VERSION-cpak.oci.tar`. Load it locally with
`podman load -i` or `docker load -i`.

## Publish

```sh
./scripts/build-cpak --push
```

`--push` publishes `ghcr.io/arkflame/flamewm-terminal:VERSION` (choose another
repository with `--image NAME`), reads the digest the registry reported and
writes `.target/dist/flamewm-terminal-VERSION-cpak.json`, the cpak v3 manifest
pinned to that digest. Manifest v3 refuses tags, so the manifest is only ever
written for a pushed image. The release workflow does this on every version tag.

cpak installs packages from a Git repository whose root holds `cpak.json`.
Commit the manifest there as `cpak.json`, then from that repository:

```sh
cpak validate cpak.json
cpak lock cpak.json
cpak test cpak.json --binary /usr/bin/flamefetch
```

and install it with `cpak install github.com/OWNER/REPOSITORY`.
