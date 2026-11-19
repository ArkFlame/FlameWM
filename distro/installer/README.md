# FlameDE installer

Two paths. Calamares is primary; `flame-install` is the fallback.

## Calamares (primary)

- Install config to `/etc/calamares/` (`settings.conf`, `modules/`), branding to `/usr/share/calamares/branding/flamede/`.
- Ext4 only (no btrfs, xfs or ZFS). Swap is zram via the optional `packages@zram` and `shellprocess@zram` pair; remove them from `settings.conf` to disable.
- `unpackfs` reads `/cdrom/casper/filesystem.squashfs`. The ISO must use the casper layout.
- Live cleanup: `removeuser` drops `flamelive`; `shellprocess@cleanup` drops `/etc/casper.conf`, `*live*` lightdm drop-ins and rebuilds the initramfs. Keep the other worker's live lightdm drop-in names matching `*live*`.
- Live-only packages removed: casper, lupin-casper, discover, laptop-detect, live-config, live-boot, ubiquity, calamares. `initramfs-tools` and `linux-generic` stay.

### Archive verification (done on the 26.10 host)

`apt-cache policy calamares` on this host (Ubuntu 26.10, stonking) reports candidate `3.3.14-0ubuntu30`, not installed. Re-run it in the ISO build environment before each release; if it reports no candidate, use `flame-install`.

## Fallback: `flame-install`

Plain bash, no TUI dependency. Partitions the disk (1 MiB bios_grub, 512 MiB ESP, ext4 root), unsquashes the live root, writes fstab by UUID, then in a chroot sets hostname, locale, timezone, user, initramfs and GRUB (EFI if the live system booted EFI, else BIOS).

- Fail-closed: refuses the live medium's disk and mounted disks, requires typing the disk path, validates inputs, and unmounts on exit.
- `--dry-run` prints mutating commands and changes nothing. A dry run still needs a real block device for `--disk`.
- Requires root and: sgdisk, partprobe, mkfs.vfat, mkfs.ext4, unsquashfs, blkid, chroot, grub-install, update-grub.

Fallback plan rationale: `ubuntu-desktop-installer` (subiquity) is heavy and Ubiquity is retired, so the fallback stays a small script.

## Checks

- `bash -n distro/installer/flame-install`: pass.
- `shellcheck distro/installer/flame-install`: pass.
- Not run: a real or dry-run install on a block device, Calamares runtime. UNVERIFIED.
