# FlameDE design: Lubuntu-style minimal Ubuntu 26.10 with FlameWM

Status: design v1, no build run. Owned paths: `distro/docs/**`, `distro/config/packages/**`,
`distro/config/system/**`. Build scripts, the local repo and the image pipeline are owned by other workers.

Target: a live and installable ISO. The desktop is FlameWM plus LightDM. LXQt, LXDE and
SDDM are not present. Base is Ubuntu 26.10 (`ubuntu-minimal`-class, no `ubuntu-desktop`).

---

## 1. Approaches evaluated

| Approach | What it is | Pros | Cons | Verdict |
|---|---|---|---|---|
| live-build (`lb`) | Debian/Ubuntu live image tool; config tree under `auto/`, `config/` | Mature, documented, one tool for ISO | Heavy config tree; Ubuntu-specific casper and installer glue is hand-maintained; slow; debootstrap-based only | Rejected as primary |
| livecd-rootfs | Ubuntu's own image builder used for official flavors | Matches Ubuntu's own flavor pipeline; hooks for seeds | Designed for Canonical infra (seeds, britney-era archive pinning, build servers); hard to run rootless; poor fit for a single-person local build | Rejected: too coupled to Canonical infra |
| mmdebstrap + casper + squashfs + GRUB hybrid | Minimal rootfs via mmdebstrap, casper live boot, squashfs(xz/zstd), xorriso hybrid ISO with GRUB (BIOS+EFI) | Small, reproducible, rootless with fakechroot/fakeroot mode, Ubuntu's own casper boot path, no extra infra | We own the glue (grub config, xorriso invocation, installer hook) | **Chosen** |

Build environment: rootless **podman** container on Ubuntu 26.10 or Debian trixie host. Inside the
container: `mmdebstrap --mode=fakechroot` (or `--mode=root` under podman user namespaces) into
`distro/build/rootfs/`, then `mksquashfs` with `-comp zstd -Xcompression-level 19` for live, and
`-comp xz` for the release artifact. The ISO is written with `xorriso -as mkisofs` with an MBR+GPT
hybrid and El Torito EFI image. Signing is out of scope for this pass; shim-signed and
grub-efi-amd64-signed are installed so Secure Boot works with the Ubuntu CA on the signed
boot chain.

Precedent for the hybrid ISO and GRUB EFI layout is the Ubuntu casper and grub packaging itself. No
path in `.vendor/` covers ISO building, so this is recorded as a design choice, not as a vendor
precedent.

---

## 2. Lubuntu parity matrix

Legend: **Kept** = same package family as Lubuntu. **Swapped** = different component, same job.
**Dropped** = not shipped.

| Area | Lubuntu (LXQt) | FlameDE choice | Status | Package(s) |
|---|---|---|---|---|
| Network | NetworkManager + nm-applet | NetworkManager + nm-applet from GTK stack | Kept | network-manager, wpasupplicant, iw, systemd-resolved (desktop.list, base.list) |
| Network GUI | nm-connection-editor (Qt-free) | nm-connection-editor from NM package | Kept | pulled by network-manager tooling |
| Audio | PulseAudio + pavucontrol-qt | PipeWire + pipewire-pulse + WirePlumber + pavucontrol (GTK) | Swapped | desktop.list; pulseaudio removed (remove.list) |
| Bluetooth | blueman | BlueZ + blueman | Kept | bluez, blueman |
| Printing | system-config-printer + CUPS | Optional; CUPS and system-config-printer; installer toggle | Kept, optional | system-config-printer (desktop.list), cups (installer cache) |
| Power | lxqt-powermanagement | UPower + power-profiles-daemon + thermald + brightnessctl | Swapped | desktop.list |
| Polkit | lxqt-policykit | polkitd + pkexec, FlameWM provides the agent | Swapped | polkitd, policykit-1 (desktop.list) |
| File manager | PCManFM-Qt | PCManFM (GTK) | Swapped | desktop.list |
| Terminal | QTerminal | foot (Wayland) + xterm (X11 fallback) | Swapped | desktop.list |
| Archive | LXQt Archiver | file-roller | Swapped | desktop.list |
| Image viewer | LXImage-Qt | gthumb | Swapped | desktop.list |
| Document viewer | (Qt viewer) | evince | Swapped | desktop.list |
| Text editor | FeatherPad | mousepad | Swapped | desktop.list |
| Calculator | (none in Lubuntu default) | galculator | Added | desktop.list |
| Screenshot | Flameshot / scrot | scrot CLI + FlameWM keybinding | Swapped | desktop.list |
| Installer | Ubiquity (Lubuntu default) | Calamares primary, ubiquity fallback | Swapped | installer.list |
| Updates | Update Manager + unattended-upgrades | Software Updater removed; unattended-upgrades kept on its own timer (image build owns it) | Partly dropped | apt-daily timers masked (mask-list.txt) |
| Fonts | Noto, DejaVu, Ubuntu fonts | Noto core, Noto CJK, Noto Color Emoji, DejaVu, Liberation, Ubuntu, Fira Code | Kept | desktop.list |
| Firmware | linux-firmware + fwupd | linux-firmware + fwupd | Kept | base.list |
| Locale | language-selector | locales + Calamares locale page | Swapped | base.list, installer.list |
| Accessibility | at-spi, Orca optional | at-spi2-core; Orca opt-in via installer | Kept | desktop.list, installer.list |
| Input methods | IBus/fcitx optional | Not shipped in base; installer language step may add | Dropped (optional) | none in lists |
| Login | SDDM (Qt) | LightDM + lightdm-gtk-greeter | Swapped | lightdm, lightdm-gtk-greeter |
| Live session | casper + Ubiquity | casper + Calamares | Swapped | live.list, installer.list |

---

## 3. What is replaced

| Lubuntu / Ubuntu default | FlameDE | Reason |
|---|---|---|
| LXQt session and panel | FlameWM (`flamewm` .deb from `distro/build/repo/`) | Product WM. Qt-free. |
| SDDM (Qt) | LightDM + lightdm-gtk-greeter | Smaller GTK stack. LightDM seat keys are verified in `.vendor/lightdm-main/src/seat.c`. |
| PulseAudio | PipeWire + WirePlumber, pipewire-pulse for compatibility | One daemon for Bluetooth and screen share. |
| NetworkManager | Kept | Layout contract. |
| snapd | Removed (remove.list) | Not wanted for a minimal desktop. Mask in mask-list.txt. |
| apport, whoopsie | Removed; masked | Crash upload is telemetry. |
| ubuntu-advantage-tools / ubuntu-pro-client | Removed; masked | Not part of the minimal base. |
| motd-news | Timer and service masked | No network fetch at login. |
| cloud-init | Removed from installed system | Desktop install, not cloud guest. Live image may keep it only if casper requires it; not expected. |
| rsyslog | Removed; journald volatile | One logger. journald Storage=volatile (`journald.conf.d/50-flamede.conf`). |
| cron | Removed; systemd timers | Ubuntu already ships timer units for apt and fstrim. |
| netplan renderer | Removed; NM renders | NM is the only network manager. |
| unattended-upgrades timer | Kept, but driven by image build | Security updates only. |
| zram-tools | Not used; systemd-zram-generator | One swap tool. |
| swap file (default) | zram swap, zstd | Lower SSD writes, faster swap on 4-8 GB. |
| lowlatency kernel | generic kernel | Lower module count, no duplicate tree. |

---

## 4. Package policy

Format of `distro/config/packages/*.list`: one package per line; `#` on its own line for comments;
no inline comments. Lists are applied in order: base, desktop, live (live image only), installer
(live image only), then remove (installed system only). Recommends are off.

- `base.list`: kernel, boot, storage tools, NetworkManager, logging and memory daemons.
- `desktop.list`: FlameWM dependencies, LightDM, PipeWire, polkit, udisks, fonts and GTK apps.
- `live.list`: casper and live-only utilities.
- `installer.list`: Calamares and its modules.
- `remove.list`: purge list for the installed system. Library packages are not listed; they go with dependents.

Unverified package names (must be checked against the 26.10 archive before the first build):

- `polkitd` vs `policykit-1`: both are listed. On noble, the daemon is `policykit-1` and `polkitd`
  exists on newer releases. If `polkitd` is absent, drop it and keep `policykit-1`.
- `calamares` and `calamares-settings-ubuntu`: see section 5.
- `fonts-firacode`, `fonts-ubuntu-console`: standard universe names; confirm.
- `systemd-zram-generator` is in `base.list`. Without it, `zram-generator.conf` is inert.

Precedent for "minimal base + opt-in" is the Ubuntu `ubuntu-minimal` seed split from `ubuntu-desktop`. No vendored reference covers Ubuntu seeds.

---

## 5. Installer

- **Primary: Calamares.** Chosen because it is distro-neutral and has a modular configuration. It is
  Qt-based, which breaks the no-Qt rule for the installed system. The exception is accepted because it
  is live-only (installer.list, removed from the target), so no Qt reaches the installed desktop.
  Verify the package in the 26.10 archive before build. Calamares is not vendored in `.vendor/`, so no
  path precedent exists; this is a design choice.
- **Fallback: ubiquity** (Ubuntu's classic installer). Used when Calamares is absent from the archive or
  fails to build. Switch: add `ubiquity` and `ubiquity-frontend-gtk` to `live.list`, remove
  `calamares*` from `installer.list`, and keep the same remove.list. Do not ship both.
- Installer hooks: LightDM autologin for `ubuntu` live user only (see section 6). The installer removes
  `lightdm.conf.d/50-flamede.conf` from the target after install, so the installed system uses the
  greeter.

---

## 6. Display manager and session

LightDM keys used in `distro/config/system/lightdm/lightdm.conf.d/50-flamede.conf`, all verified in
`.vendor/lightdm-main/src/seat.c` and defaulted in `.vendor/lightdm-main/src/lightdm.c`:

| Key | Value | Evidence |
|---|---|---|
| `user-session` | `flamewm` | seat.c:1034, 1074, 1179; lightdm.c:809 default |
| `greeter-session` | `lightdm-gtk-greeter` | seat.c:1253; lightdm.c:807 default |
| `autologin-user` | `ubuntu` (live only) | seat.c:1169, 1300 |
| `autologin-session` | `flamewm` | seat.c:1171, 1303 |
| `autologin-user-timeout` | `0` | seat.c:1306, 1710 |
| `greeter-hide-users` | `true` | seat.c:578 (greeter hint) |
| `greeter-show-manual-login` | `true` | seat.c:579 |
| `allow-guest` | `false` | seat.c:383 |

Greeter fallback: if `lightdm-gtk-greeter` misbehaves on the target GPU, switch
`greeter-session` to `slick-greeter` (Ubuntu universe package, not in lists; add only if the fallback
is taken). Do not ship both.

The installed system has no autologin. The installer removes the live drop-in, so first boot shows
the greeter.

---

## 7. Services and tuning

| Area | Choice | File |
|---|---|---|
| Swap | zram 50% RAM, cap 8 GiB, zstd, priority 100 | `systemd/zram-generator.conf` |
| VM tuning | swappiness 100, vfs_cache_pressure 50, dirty 5/15, page-cluster 0 | `sysctl.d/99-flamede.conf` |
| Watchdogs | kernel NMI and lockup watchdogs off; systemd hardware watchdog off; `iTCO_wdt` and related modules blocked | `sysctl.d`, `system.conf.d/50-flamede.conf`, `modprobe.d/flamede-blacklist.conf` |
| Logging | journald volatile, RuntimeMaxUse 64M | `systemd/journald.conf.d/50-flamede.conf` |
| Shutdown timeouts | DefaultTimeoutStopSec 10s | `systemd/system.conf.d/50-flamede.conf` |
| Accounting | Memory, CPU and IO accounting off by default | `systemd/system.conf.d/50-flamede.conf` |
| OOM | earlyoom | `packages/base.list` |
| I/O scheduler | NVMe none; SATA SSD and eMMC mq-deadline; HDD bfq | `udev/rules.d/60-flamede-iosched.rules` |
| Timers | apt-daily and motd-news masked; fstrim kept | `mask-list.txt` |

Note on `vm.swappiness=100`: this is the recommended value when swap is zram. It is set deliberately,
not as a default.

---

## 8. Risks and open items

1. Calamares archive presence: unverified. Fallback in section 5.
2. `systemd-zram-generator` is in `base.list` (added this pass). Its config installs to `/etc/systemd/zram-generator.conf`; the image build must copy it there.
3. Live user name `ubuntu` assumed from casper defaults. Confirm in the built image.
4. Mask list includes `chrony.service` and `rsyslog.service`. Harmless if absent.
5. `DefaultMemoryAccounting=no` breaks `systemd-run --property=MemoryMax`. Accept, document for users.
6. No build was run in this pass. All claims about the archive are UNVERIFIED.
