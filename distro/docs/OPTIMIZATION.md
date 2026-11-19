# FlameDE footprint optimization

Every item is [T] theoretical until boot-smoke.sh measures a built ISO. Payoffs are order-of-magnitude estimates from upstream docs, not benchmarks. Gate each change with `distro/tests/boot-smoke.sh` then `check-budget.py`.

## RAM
- [T] zram swap, zstd: `systemd-zram-generator`, `zram-size = ram / 2`, `compression-algorithm = zstd`. Idle RSS unchanged; ~1-2 GB effective headroom on 4-8 GB under pressure. Measure swap use under load.
- [T] earlyoom (in base.list): ~2 MB RSS; set `-m 5 -s 10` thresholds. Prevents hard freezes; no idle cost.
- [T] MALLOC_ARENA_MAX=2 (glibc) in session env: tens of MB saved in multithreaded apps; cost is lock contention in heavy threaded workloads.
- [T] THP `madvise` (verify Ubuntu default with `cat /sys/kernel/mm/transparent_hugepage/enabled`): avoids khugepaged CPU and RSS bloat.
- [T] KSM: keep off. Benefit is VM-only; costs CPU on desktop.
- [T] journald `Storage=volatile`, `RuntimeMaxUse=32M`: no disk writes, bounded RAM. Measure `journalctl --disk-usage`.
- [T] Remove unneeded daemons (Python/Java services) from default session: each typically 20-40 MB. Audit with `ps -eo rss,comm --sort=-rss`.
- [T] dbus-broker instead of dbus-daemon: a few MB and fewer wakeups. Must be tested with LightDM, NM and polkit first.
- [T] One audio stack: pipewire + pipewire-pulse, no pulseaudio. Compare RSS on live session before choosing.
- [T] Do not install preload: continuous background RAM and I/O for little gain.
- [T] tmpfs /tmp, `size=50%`: removes disk writes; RAM-backed, so large builds need the cap.

## Boot and CPU
- [T] Kernel cmdline `nowatchdog`: drops watchdog and NMI-watchdog timers, small idle wakeup saving, no security cost. Apply via GRUB_CMDLINE_LINUX_DEFAULT.
- [T] `mitigations=off`: up to ~10-15% on syscall-heavy load on older CPUs. Security regression; keep default, opt-in only.
- [T] `no_console_suspend`: debug aid, zero payoff. Keep out of release.
- [T] Mask apt-daily*, motd-news, man-db timers: fewer wakeups and I/O bursts. Trade-off: updates must be scheduled elsewhere.
- [T] Remove snapd, cloud-init, apport, whoopsie, ubuntu-advantage (remove, not just mask): cloud-init and snapd add boot seconds and tens of MB RSS.
- [T] Keep systemd-resolved (NM integration). Dropping it saves ~5-10 MB but breaks split DNS.
- [T] I/O scheduler: `none` for NVMe, `mq-deadline` for SATA SSD, `bfq` for rotational. Latency gain, no RAM.
- [T] Locale and fonts: one locale, no locales-all, no fonts-noto-cjk unless required. Mostly ISO size; RAM only when used.

## Image size and boot pipeline
- [T] initramfs-tools `MODULES=dep`: ~20-40 MB smaller initrd, faster boot. Risk: hardware not detected on unseen machines. Use on installed system only, or test on a hardware matrix first.
- [T] squashfs: zstd (level 15, 1M blocks) decompresses several times faster than xz but yields a larger ISO (typically 10-30%). Choose zstd if the boot budget binds, xz if the size budget binds. Measure both with boot-smoke.sh.
- [T] Kernel: keep linux-generic (lowlatency rejected in base.list, saves duplicate modules).

## Measured results (2026-11-19, test ISO, qemu 1G, KVM)

| metric | baseline | after batch | tag |
|---|---|---|---|
| ISO size | 1544.0 MB | 1524.6 MB | [M] |
| wall to LIGHTDM_ACTIVE | 26.27 s | 26.22 s | [M] |
| services running | 17 | 16 | [M] |
| used RAM at probe | 264 MB (sampled ~17 s) | 359 MB (sampled ~105 s, after autologin) | [M] not comparable |
| systemd state | starting | timeout (never running within 90 s) | [M] open |

- Probe fix: state line now bounded wait; still reports non-running. Pre-existing: baseline was also `starting`.
- Top RSS after batch: Xorg 109 MB, flamewm-desktop 33, flamewm-shell 22, wireplumber 22, NM 19 (blueman gone).
- blame top: ldconfig 2.4 s, dev-sr0 1.2 s, gpu-manager 0.4 s. critical-chain empty: the stuck unit is not yet identified.
- Items applied: drop blueman, thermald, fwupd, avahi-daemon [T] (ISO -19 MB [M], RSS of each [M] only for blueman); masks [T]; grub `nowatchdog nmi_watchdog=0`, drop `zswap.enabled=1` [T]. zram kept over zswap.
- Rejected: INITRAMFS_MODULES=dep [T] (hardware-safety risk, kept `most`); squashfs xz [T] (ISO larger, kept zstd).
- Budget tightened: size 1680, wall 29, services 18. RAM budget kept at 900: the baseline sample point differs, so no fair tightening yet.
- Next: identify the unit blocking systemd `running` (add `systemctl list-jobs` and `--failed` to the probe), then re-measure RAM at the same sample point.
