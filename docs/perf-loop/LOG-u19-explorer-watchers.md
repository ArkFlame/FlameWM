# u19-explorer-watchers changes

Second pass over the watcher/places/ark-extract area. Earlier units had already made the
directory watch fully event-driven (blocking `poll`, 200 ms coalescing) and moved `lsblk` and
`udisks` work to background threads, so this pass targets the remaining per-call allocations and
the last periodic wakeup in the area.

- dndext.rs: the external `gdbus` wait is now event-driven. A pidfd for the child is polled, so a
  multi-second `org.kde.ark.DndExtract` call no longer wakes the helper thread every 10 ms
  (100 wakeups/s) and a finished child is reaped as soon as the kernel reports it instead of on
  the next 10 ms tick. The poll timeout is `min(remaining, 50 ms)`, which only bounds cancel
  latency (~20 wakeups/s worst case); kernels without `pidfd_open` fall back to the old timed
  loop unchanged. Timeout, cancel and message behaviour are identical.
- dndext.rs: `gvariant_string` escapes in one pass into a pre-sized buffer instead of
  `str::replace` twice plus `format!` (three allocations plus two full copies per call).
- places.rs: `parse_mountinfo` and `parse_network_mounts` scan each mountinfo line in one pass
  instead of collecting a `Vec<&str>` per line, removing one heap allocation per line on every
  places refresh (mountinfo is read on start-up and on every udev/mount event).
- places.rs: both parsers deduplicate with a linear scan over the (tiny) result list instead of
  allocating a `HashSet` and hashing every candidate mount point per refresh.
- places.rs: `is_system_partition` compares the partition type GUID / MBR byte and the label with
  `eq_ignore_ascii_case` against the constant tables, dropping two `to_ascii_lowercase` `String`
  allocations per `lsblk` node while keeping the case-insensitive match.
- refresh.rs: the watch thread drains the entire inotify queue per wakeup (loop until
  `WouldBlock`) instead of reading one 4 KiB buffer and returning to `poll` for each chunk of a
  large burst, so a big copy no longer rebuilds the poll set and re-reads the clock once per
  4 KiB of events.

Expected composition shift: ark drag-out spends no CPU while an external extraction runs (was
100 wakeups/s for up to 5 s) and reacts on child exit immediately; places refresh allocates a few
hundred bytes instead of one vector per mountinfo line, one hash set and two lowercase strings per
lsblk node; large directory bursts cost one poll round instead of one per 4 KiB of events.
Visuals, entries and their ordering are unchanged.
