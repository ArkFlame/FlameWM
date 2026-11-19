# FlameDE live ISO

Ubuntu-based minimal live ISO with FlameWM. Built rootless inside a podman container.

## Usage

    make -C distro test     # bash -n (+ shellcheck if installed)
    make -C distro deb      # flamewm .deb -> distro/build/repo (runs scripts/build-deb if missing)
    make -C distro image    # builder image (ubuntu:26.10 base)
    make -C distro iso      # -> distro/out/FlameDE-<version>-amd64.iso + .sha256
    make -C distro clean    # removes distro/out and the work dir (podman unshare rm)

Fallback base: `UBUNTU_IMAGE=ubuntu:rolling make -C distro image iso`.

Tuning (environment, forwarded into the container):
- `SQUASH_COMP=zstd|xz`, `ZSTD_LEVEL=19`
- `INITRAMFS_MODULES=most|dep` (default `most`; `dep` is smaller but drops drivers for unknown hardware)
- `MIRROR`, `SUITE` (default: the container's Ubuntu codename), `COMPONENTS`
- `MMDEB_MODE=unshare|fakechroot` (fallback if user namespaces are unavailable)

## Inputs

- `distro/config/packages/{base,desktop,live,installer}.list`: install set (comma-joined, `#` comments ok)
- `distro/config/packages/remove.list`: purged inside the chroot after install
- `distro/config/system/**`: copied into rootfs; `mask-list.txt` is not copied, its units are symlinked to `/dev/null`
- `distro/overlay/**`: copied into rootfs last (owned by the installer work)
- `distro/build/repo/*.deb`: local unsigned apt repo (`trusted=yes`), removed from the final image

## Pipeline

1. mmdebstrap minbase + package lists + local repo, then purge `remove.list`
2. config/system subtrees mapped to /etc (sysctl, journald, system.conf, zram, modprobe, udev, lightdm), `distro/overlay` copied, installer payload, masks, live user `flamelive` (no password). `TEST_PROBE=1` adds the guest probe and tags the ISO `-test`.
3. casper initramfs (`COMPRESS=zstd`), kernel and initrd copied out
4. cleanup: apt lists, docs (keeps copyright), man, non-en locales, logs, local repo
5. `filesystem.manifest`, `filesystem.size`, `filesystem.squashfs`
6. `grub-mkrescue` hybrid BIOS+UEFI ISO, boot args: `boot=casper quiet systemd.show_status=0 zswap.enabled=1 zram.num_devices=1`

## Requirements

- amd64 host with podman, rootless user namespaces, and enough disk (about 20 GB free for the work dir)
- No sudo needed. The wrapper runs podman with `seccomp=unconfined`, `apparmor=unconfined`, `SYS_ADMIN`.

## Not yet verified

- A full ISO build has not been run. Expect first-run issues in mmdebstrap unshare mode on the target host.
- `distro/out/` and `.target/` should be git-ignored; `.target/` already is, `distro/out/` is not yet.
